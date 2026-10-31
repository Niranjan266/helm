package com.niranjan.helm.core

import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Proxy
import java.net.Socket
import java.net.SocketAddress
import java.net.SocketException
import java.net.SocketTimeoutException
import java.util.ArrayDeque
import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock
import javax.net.SocketFactory
import kotlin.concurrent.withLock
import okhttp3.Dns
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString

/**
 * A [Socket] whose byte stream is one relay tunnel (relay/README.md §2): `connect()` opens
 * `wss://<relay>/connect/<host_id>`, writes become binary messages of ≤ 64 KiB, received binary
 * messages are the input stream. OkHttp layers the pinned TLS + Helm WebSocket on top unchanged.
 */
class TunnelSocket(
    private val relayClient: OkHttpClient,
    private val connectUrl: String,
) : Socket() {
    private val lock = ReentrantLock()
    private val readable = lock.newCondition()
    private val opened = lock.newCondition()
    private val chunks = ArrayDeque<ByteArray>()
    private var head: ByteArray? = null
    private var headPos = 0

    @Volatile private var ws: WebSocket? = null
    @Volatile private var open = false
    @Volatile private var eof = false
    @Volatile private var failure: IOException? = null
    @Volatile private var closed = false
    @Volatile private var inShut = false
    @Volatile private var outShut = false
    @Volatile private var timeoutMs = 0
    @Volatile private var remote: SocketAddress? = null

    /** Close code from the relay, when it closed the tunnel. */
    @Volatile var closeCode: Int? = null
        private set

    private val listener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) = lock.withLock {
            open = true
            opened.signalAll()
        }

        override fun onMessage(webSocket: WebSocket, bytes: ByteString) = lock.withLock {
            if (bytes.size > 0) chunks.addLast(bytes.toByteArray())
            readable.signalAll()
        }

        override fun onMessage(webSocket: WebSocket, text: String) = Unit // `pong` keep-alive

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
            finish(code, null)
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = finish(code, null)

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            val msg = response?.code?.let { TunnelProtocol.httpReason(it) } ?: "Relay connection failed: ${t.message ?: t.javaClass.simpleName}"
            finish(null, IOException(msg, t))
        }
    }

    private fun finish(code: Int?, error: IOException?) = lock.withLock {
        if (code != null && closeCode == null) closeCode = code
        if (error != null && failure == null) failure = error
        if (code != null && code != 1000 && failure == null && !open) failure = IOException(TunnelProtocol.closeReason(code))
        eof = true
        readable.signalAll()
        opened.signalAll()
    }

    override fun connect(endpoint: SocketAddress?) = connect(endpoint, 0)

    override fun connect(endpoint: SocketAddress?, timeout: Int) {
        if (closed) throw SocketException("Socket is closed")
        if (ws != null) throw SocketException("already connected")
        remote = endpoint
        ws = relayClient.newWebSocket(Request.Builder().url(connectUrl).build(), listener)
        val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(if (timeout > 0) timeout.toLong() else 15_000)
        lock.withLock {
            while (!open && !eof) {
                val left = deadline - System.nanoTime()
                if (left <= 0) break
                opened.awaitNanos(left)
            }
        }
        if (!open) {
            ws?.cancel()
            throw failure ?: SocketTimeoutException("Relay did not answer in time.")
        }
    }

    private val input = object : InputStream() {
        override fun read(): Int {
            val one = ByteArray(1)
            return if (read(one, 0, 1) < 0) -1 else one[0].toInt() and 0xff
        }

        override fun read(b: ByteArray, off: Int, len: Int): Int {
            if (len == 0) return 0
            lock.withLock {
                val deadline = if (timeoutMs > 0) System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(timeoutMs.toLong()) else 0L
                while (true) {
                    val h = head
                    if (h != null && headPos < h.size) {
                        val n = minOf(len, h.size - headPos)
                        System.arraycopy(h, headPos, b, off, n)
                        headPos += n
                        return n
                    }
                    val next = chunks.pollFirst()
                    if (next != null) {
                        head = next
                        headPos = 0
                        continue
                    }
                    if (closed) throw SocketException("Socket closed")
                    if (eof || inShut) {
                        failure?.let { throw it }
                        return -1
                    }
                    if (deadline == 0L) {
                        readable.await()
                    } else {
                        val left = deadline - System.nanoTime()
                        if (left <= 0) throw SocketTimeoutException("Read timed out")
                        readable.awaitNanos(left)
                    }
                }
            }
        }

        override fun available(): Int = lock.withLock { (head?.size ?: 0) - headPos + chunks.sumOf { it.size } }

        override fun close() = this@TunnelSocket.close()
    }

    private val output = object : OutputStream() {
        override fun write(b: Int) = write(byteArrayOf(b.toByte()), 0, 1)

        override fun write(b: ByteArray, off: Int, len: Int) {
            val socket = ws ?: throw SocketException("not connected")
            if (closed || outShut) throw SocketException("Socket closed")
            for (chunk in TunnelProtocol.chunks(b, off, len)) {
                // Bounded queue: OkHttp fails the socket past 16 MiB.
                var waited = 0
                while (socket.queueSize() > QUEUE_LIMIT) {
                    if (eof || closed) throw SocketException("Relay tunnel closed")
                    Thread.sleep(5)
                    waited += 5
                    if (timeoutMs in 1..waited) throw SocketTimeoutException("Write timed out")
                }
                if (eof || !socket.send(chunk.toByteString())) {
                    throw failure ?: SocketException("Relay tunnel closed")
                }
            }
        }

        override fun close() = this@TunnelSocket.close()
    }

    override fun getInputStream(): InputStream {
        if (!isConnected) throw SocketException("not connected")
        return input
    }

    override fun getOutputStream(): OutputStream {
        if (!isConnected) throw SocketException("not connected")
        return output
    }

    override fun close() {
        lock.withLock {
            if (closed) return
            closed = true
            readable.signalAll()
            opened.signalAll()
        }
        ws?.close(1000, null)
    }

    override fun shutdownInput() {
        inShut = true
        lock.withLock { readable.signalAll() }
    }

    override fun shutdownOutput() {
        outShut = true
    }

    override fun isConnected(): Boolean = open
    override fun isBound(): Boolean = open
    override fun isClosed(): Boolean = closed
    override fun isInputShutdown(): Boolean = inShut
    override fun isOutputShutdown(): Boolean = outShut
    override fun setSoTimeout(timeout: Int) { timeoutMs = timeout.coerceAtLeast(0) }
    override fun getSoTimeout(): Int = timeoutMs
    override fun setTcpNoDelay(on: Boolean) = Unit
    override fun getTcpNoDelay(): Boolean = true
    override fun setKeepAlive(on: Boolean) = Unit
    override fun setSoLinger(on: Boolean, linger: Int) = Unit
    override fun setSendBufferSize(size: Int) = Unit
    override fun setReceiveBufferSize(size: Int) = Unit
    override fun getRemoteSocketAddress(): SocketAddress? = remote
    override fun getInetAddress(): InetAddress? = (remote as? InetSocketAddress)?.address
    override fun getPort(): Int = (remote as? InetSocketAddress)?.port ?: 0
    override fun getLocalPort(): Int = -1
    override fun getLocalAddress(): InetAddress = InetAddress.getLoopbackAddress()
    override fun getLocalSocketAddress(): SocketAddress? = null
    override fun toString(): String = "TunnelSocket[$connectUrl]"

    private companion object {
        const val QUEUE_LIMIT = 1L * 1024 * 1024
    }
}

/** Hands OkHttp a fresh [TunnelSocket] per connection. */
class TunnelSocketFactory(private val relayClient: OkHttpClient, private val connectUrl: String) : SocketFactory() {
    override fun createSocket(): Socket = TunnelSocket(relayClient, connectUrl)
    override fun createSocket(host: String?, port: Int): Socket = createSocket().also { it.connect(InetSocketAddress.createUnresolved(host, port)) }
    override fun createSocket(host: String?, port: Int, localHost: InetAddress?, localPort: Int): Socket = createSocket(host, port)
    override fun createSocket(host: InetAddress?, port: Int): Socket = createSocket().also { it.connect(InetSocketAddress(host, port)) }
    override fun createSocket(address: InetAddress?, port: Int, localAddress: InetAddress?, localPort: Int): Socket = createSocket(address, port)
}

/** The dummy inner host never hits DNS: any name maps to a placeholder (ignored by the tunnel). */
object RelayDns : Dns {
    override fun lookup(hostname: String): List<InetAddress> =
        listOf(InetAddress.getByAddress(hostname, byteArrayOf(127, 0, 0, 1)))
}

/**
 * Runs the normal pinned TLS + `/ws` (or `/stream`) inside a relay tunnel (§12). [inner] supplies
 * the pinned client; this only swaps in the tunnel socket factory and the dummy DNS. Pairing is
 * never allowed over it ([relayed] = true).
 */
class RelayTransport(
    val connectUrl: String,
    private val relayClient: OkHttpClient = OkHttpClient.Builder().readTimeout(0, TimeUnit.MILLISECONDS).build(),
    private val inner: HelmTransport = PinnedTlsTransport(connectTimeoutMs = 15_000),
) : HelmTransport {
    override val scheme: String get() = inner.scheme
    override val relayed: Boolean get() = true

    override fun client(fingerprintHex: String): OkHttpClient = inner.client(fingerprintHex).newBuilder()
        .socketFactory(TunnelSocketFactory(relayClient, connectUrl))
        .dns(RelayDns)
        .proxy(Proxy.NO_PROXY)
        .retryOnConnectionFailure(false)
        .build()

    companion object {
        /** Null when the PC has no usable relay URL. */
        fun forPc(relayUrl: String?, hostId: String): RelayTransport? =
            TunnelProtocol.connectUrl(relayUrl, hostId)?.let { RelayTransport(it) }
    }
}
