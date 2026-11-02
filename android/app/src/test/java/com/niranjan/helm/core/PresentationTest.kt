package com.niranjan.helm.core

import java.net.InetAddress
import java.util.UUID
import java.util.concurrent.TimeUnit
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.sin
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** G2 presentation mode: laser frames, tilt → coordinates, timer, state parsing, gating. */
class PresentationTest {
    private fun obj(s: String) = Json.parseToJsonElement(s) as JsonObject
    private val eps = 1e-4f

    /** Rotation-vector values for a quaternion rotating by [angle] around unit axis (ax, ay, az). */
    private fun rv(ax: Double, ay: Double, az: Double, angle: Double): FloatArray {
        val s = sin(angle / 2)
        return floatArrayOf((ax * s).toFloat(), (ay * s).toFloat(), (az * s).toFloat(), cos(angle / 2).toFloat())
    }

    private fun angles(v: FloatArray) = TiltPointer.pointing(TiltPointer.matrixFromRotationVector(v))

    @Test
    fun laserFramesHaveTheWireShape() {
        assertEquals(obj("""{"type":"in","k":"lp","x":0.25,"y":0.75,"on":true}"""), LaserFrame(0.25f, 0.75f, true).toJson())
        assertEquals(obj("""{"type":"in","k":"lp","x":1.0,"y":0.0,"on":true}"""), LaserFrame(3f, -1f, true).toJson())
        assertEquals(obj("""{"type":"in","k":"lp","on":false}"""), LaserFrame.OFF.toJson())
        assertEquals(obj("""{"type":"in","k":"lp","x":0.1235,"y":0.5,"on":true}"""), LaserFrame(0.123456f, 0.5f, true).toJson())
    }

    @Test
    fun rotationVectorGivesWhereTheTopEdgePoints() {
        // Flat, top edge to the north: heading 0, level.
        val (y0, p0) = angles(floatArrayOf(0f, 0f, 0f, 1f))
        assertEquals(0.0, y0, 1e-6)
        assertEquals(0.0, p0, 1e-6)
        // Turned 30° to the right (clockwise seen from above = negative about +z).
        val (y1, p1) = angles(rv(0.0, 0.0, 1.0, -PI / 6))
        assertEquals(PI / 6, y1, 1e-5)
        assertEquals(0.0, p1, 1e-5)
        // Top tilted up 20° (positive about +x, the phone's right edge).
        val (y2, p2) = angles(rv(1.0, 0.0, 0.0, PI / 9))
        assertEquals(0.0, y2, 1e-5)
        assertEquals(PI / 9, p2, 1e-5)
        // Without w the sensor's w is reconstructed.
        val (y3, _) = angles(rv(0.0, 0.0, 1.0, -PI / 6).copyOf(3))
        assertEquals(PI / 6, y3, 1e-5)
        // Matches SensorManager's layout for a known quaternion.
        val m = TiltPointer.matrixFromRotationVector(rv(0.0, 0.0, 1.0, PI / 2))
        assertArrayEquals(doubleArrayOf(0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0), m, 1e-6)
    }

    @Test
    fun tiltMapsRelativeToCalibration() {
        val t = TiltPointer(spanYawDeg = 40f, spanPitchDeg = 20f)
        assertEquals(0.5f to 0.5f, t.position(1.0, 1.0)) // not calibrated yet
        val yaw0 = Math.toRadians(170.0)
        t.calibrate(yaw0, 0.1)
        assertTrue(t.isCalibrated)
        val (cx, cy) = t.position(yaw0, 0.1)
        assertEquals(0.5f, cx, eps)
        assertEquals(0.5f, cy, eps)
        // 10° right = a quarter of the 40° span; heading wraps past 180°.
        val (rx, _) = t.position(Math.toRadians(-180.0), 0.1)
        assertEquals(0.75f, rx, eps)
        // 5° up = a quarter of the 20° span, up = smaller y.
        val (_, uy) = t.position(yaw0, 0.1 + Math.toRadians(5.0))
        assertEquals(0.25f, uy, eps)
        // Far left / far down clamp to the edge.
        assertEquals(0f to 1f, t.position(yaw0 - 1.5, -1.0))
        t.reset()
        assertFalse(t.isCalibrated)
        assertEquals(PI - 0.1, TiltPointer.wrap(-PI - 0.1), 1e-9)
        assertEquals(0.0, TiltPointer.wrap(4 * PI), 1e-9)
    }

    @Test
    fun smoothingThrottleAndPad() {
        val s = LaserSmoother(alpha = 0.5f)
        assertEquals(0.25f to 0.25f, s.push(0.25f, 0.25f))
        assertEquals(0.5f to 0.375f, s.push(0.75f, 0.5f))
        s.reset()
        assertEquals(0.9f to 0.1f, s.push(0.9f, 0.1f))

        val th = LaserThrottle(minDelta = 0.01f)
        assertTrue(th.shouldSend(0.5f, 0.5f))
        assertFalse(th.shouldSend(0.505f, 0.495f))
        assertTrue(th.shouldSend(0.52f, 0.5f))
        th.reset()
        assertTrue(th.shouldSend(0.52f, 0.5f))

        assertEquals(0.25f to 0.5f, padToLaser(100f, 90f, 400f, 180f))
        assertEquals(1f to 0f, padToLaser(500f, -5f, 400f, 180f))
        assertEquals(0.5f to 0.5f, padToLaser(1f, 1f, 0f, 0f))
    }

    @Test
    fun timerCountsPausesAndAlertsOnce() {
        val t = PresentTimer(countdownS = 10 * 60)
        assertNull(PresentTimer().remainingMs(0))
        t.start(1_000)
        assertEquals(60_000L, t.elapsedMs(61_000))
        t.pause(61_000)
        assertEquals(60_000L, t.elapsedMs(500_000))
        assertFalse(t.running)
        t.start(500_000)
        // 5 min left at 5 min elapsed.
        assertEquals(emptyList<Int>(), t.dueAlerts(500_000 + 239_000))
        assertEquals(listOf(300), t.dueAlerts(500_000 + 240_000))
        assertEquals(emptyList<Int>(), t.dueAlerts(500_000 + 241_000))
        // Skipping ahead past both remaining marks reports both, largest first.
        assertEquals(listOf(60, 0), t.dueAlerts(500_000 + 600_000))
        assertEquals(-60_000L, t.remainingMs(500_000 + 600_000))
        t.reset()
        assertEquals(0L, t.elapsedMs(9_999_999))
        // Marks longer than the countdown never fire.
        val short = PresentTimer(countdownS = 120)
        short.start(0)
        assertEquals(listOf(60), short.dueAlerts(60_000))
        assertEquals(listOf(0), short.dueAlerts(120_000))

        assertEquals("0:00", PresentTimer.format(0))
        assertEquals("4:05", PresentTimer.format(245_900))
        assertEquals("1:00:01", PresentTimer.format(3_601_000))
        assertEquals("-1:30", PresentTimer.format(-90_000))
        assertTrue(PresentTimer.vibration(0).size > PresentTimer.vibration(300).size)
    }

    @Test
    fun stateParses() {
        val s = PresentState.from(obj("""{"app":"powerpoint","title":"PowerPoint Slide Show - Q3","exe":"POWERPNT.EXE",
            "slide":{"index":3,"count":12,"notes":"Margins","showing":true},"slide_status":"ok",
            "laser":{"visible":true,"monitor_id":null}}"""))
        assertEquals("PowerPoint", s.appLabel)
        assertEquals(SlideInfo(3, 12, "Margins", true), s.slide)
        assertEquals("3 / 12", s.slide!!.label)
        assertTrue(s.laserVisible)
        val none = PresentState.from(obj("""{"app":null,"title":null,"slide":null,"slide_status":"unsupported","laser":{"visible":false}}"""))
        assertNull(none.app)
        assertNull(none.slide)
        assertEquals("unsupported", none.slideStatus)
        assertEquals("– / 5", SlideInfo(0, 5, "", true).label)
        assertEquals(listOf("next", "prev", "first", "last", "black", "start", "end"), PresentAction.entries.map { it.wire })
    }

    @Test
    fun callsAreGatedOnG2() = runBlocking {
        val server = MockWebServer()
        val agent = FakeAgent(features = listOf("p1", "p2", "p3", "p4", "p5"))
        val plain = object : HelmTransport {
            override val scheme = "ws"
            override fun client(fingerprintHex: String): OkHttpClient = OkHttpClient.Builder().connectTimeout(4, TimeUnit.SECONDS).build()
        }
        server.enqueue(MockResponse().withWebSocketUpgrade(agent.listener()))
        server.start(InetAddress.getByName("127.0.0.1"), 0)
        val conn = HelmConnection(
            agent.hostId, "ab".repeat(32), listOf(Endpoint("127.0.0.1", server.port)),
            DeviceIdentity(UUID.randomUUID().toString(), "Test Phone", SoftwareDeviceSigner()),
            HandshakeMode.Pair(agent.validToken), plain, 5_000, 5_000, 5_000,
        )
        conn.connect()
        val api = HelmApi(conn)
        for (call in listOf<suspend () -> Unit>({ api.presentState() }, { api.presentGo(PresentAction.NEXT) }, { api.presentLaser(null, true) })) {
            try {
                call()
                fail("expected unsupported")
            } catch (e: HelmException) {
                assertEquals(LocalCodes.UNSUPPORTED, e.code)
            }
        }
        assertTrue(agent.p3Calls.none { it.first.startsWith("present.") })
        conn.close()
        server.shutdown()
    }
}
