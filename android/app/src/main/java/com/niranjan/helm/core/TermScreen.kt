package com.niranjan.helm.core

import java.nio.ByteBuffer
import java.nio.CharBuffer
import java.nio.charset.CodingErrorAction

/** Cell colours: 0 = terminal default, otherwise opaque ARGB. */
data class TermStyle(val fg: Int = 0, val bg: Int = 0, val bold: Boolean = false, val inverse: Boolean = false, val underline: Boolean = false) {
    companion object {
        val DEFAULT = TermStyle()
    }
}

/** A run of same-styled text in one line. */
data class TermRun(val text: String, val style: TermStyle)

/**
 * A small VT100/xterm screen for ConPTY output (§13 `term.output`): UTF-8 decoding across
 * chunks, cursor movement, erase, insert/delete, SGR colours (16, 256 and 24-bit), scroll
 * region, alternate screen, and scrollback. Everything else (OSC titles, DEC modes we do not
 * use, charset selects) is parsed and dropped. Not thread-safe; feed from one thread.
 */
class TermScreen(cols: Int = 80, rows: Int = 24, private val scrollbackMax: Int = 2000) {
    var cols = cols.coerceAtLeast(1)
        private set
    var rows = rows.coerceAtLeast(1)
        private set

    private class Cell(var ch: Char = ' ', var style: TermStyle = TermStyle.DEFAULT)

    private fun blankLine(n: Int = cols) = Array(n) { Cell() }

    private var screen: MutableList<Array<Cell>> = MutableList(this.rows) { blankLine() }
    private val scrollback = ArrayDeque<Array<Cell>>()
    private var savedMain: MutableList<Array<Cell>>? = null

    var cursorRow = 0
        private set
    var cursorCol = 0
        private set
    var cursorVisible = true
        private set

    /** Bumped on every change; the UI redraws when it moves. */
    var version = 0L
        private set

    private var style = TermStyle.DEFAULT
    private var pendingWrap = false
    private var savedRow = 0
    private var savedCol = 0
    private var top = 0
    private var bottom = this.rows - 1

    private val decoder = Charsets.UTF_8.newDecoder()
        .onMalformedInput(CodingErrorAction.REPLACE)
        .onUnmappableCharacter(CodingErrorAction.REPLACE)
    private var leftover = ByteArray(0)

    private enum class St { GROUND, ESC, CSI, OSC, OSC_ESC, CHARSET }

    private var st = St.GROUND
    private val csi = StringBuilder()

    fun feed(bytes: ByteArray) {
        val all = if (leftover.isEmpty()) bytes else leftover + bytes
        val inBuf = ByteBuffer.wrap(all)
        val out = CharBuffer.allocate(all.size + 1)
        decoder.decode(inBuf, out, false)
        leftover = ByteArray(inBuf.remaining()).also { inBuf.get(it) }
        out.flip()
        feed(out)
    }

    fun feed(text: CharSequence) {
        for (c in text) step(c)
        version++
    }

    private fun step(c: Char) {
        when (st) {
            St.GROUND -> ground(c)
            St.ESC -> esc(c)
            St.CSI -> {
                if (c in '@'..'~' && !(csi.isEmpty() && c in "<=>?")) {
                    csiDispatch(csi.toString(), c)
                    csi.setLength(0)
                    st = St.GROUND
                } else if (c == '\u001b') {
                    csi.setLength(0)
                    st = St.ESC
                } else if (csi.length < 64) {
                    csi.append(c)
                }
            }
            St.OSC -> when (c) {
                '\u0007' -> st = St.GROUND
                '\u001b' -> st = St.OSC_ESC
                else -> Unit
            }
            St.OSC_ESC -> st = if (c == '\\') St.GROUND else St.OSC
            St.CHARSET -> st = St.GROUND
        }
    }

    private fun ground(c: Char) {
        when (c) {
            '\u001b' -> st = St.ESC
            '\r' -> { cursorCol = 0; pendingWrap = false }
            '\n', '\u000b', '\u000c' -> { lineFeed(); pendingWrap = false }
            '\b' -> { if (cursorCol > 0) cursorCol--; pendingWrap = false }
            '\t' -> cursorCol = minOf(cols - 1, (cursorCol / 8 + 1) * 8)
            '\u0007', '\u0000', '\u000e', '\u000f' -> Unit
            else -> if (c >= ' ') put(c)
        }
    }

    private fun esc(c: Char) {
        st = St.GROUND
        when (c) {
            '[' -> { csi.setLength(0); st = St.CSI }
            ']' -> st = St.OSC
            '(', ')', '*', '+' -> st = St.CHARSET
            '7' -> saveCursor()
            '8' -> restoreCursor()
            'M' -> reverseIndex()
            'D' -> lineFeed()
            'E' -> { cursorCol = 0; lineFeed() }
            'c' -> reset()
            else -> Unit
        }
    }

    private fun put(c: Char) {
        if (pendingWrap) {
            cursorCol = 0
            lineFeed()
            pendingWrap = false
        }
        val cell = screen[cursorRow][cursorCol]
        cell.ch = c
        cell.style = style
        if (cursorCol == cols - 1) pendingWrap = true else cursorCol++
    }

    private fun lineFeed() {
        if (cursorRow == bottom) scrollUp(1) else if (cursorRow < rows - 1) cursorRow++
    }

    private fun reverseIndex() {
        if (cursorRow == top) scrollDown(1) else if (cursorRow > 0) cursorRow--
    }

    private fun scrollUp(n: Int) {
        repeat(n.coerceAtMost(bottom - top + 1)) {
            val gone = screen.removeAt(top)
            if (top == 0 && savedMain == null) {
                scrollback.addLast(gone)
                while (scrollback.size > scrollbackMax) scrollback.removeFirst()
            }
            screen.add(bottom, blankLine())
        }
    }

    private fun scrollDown(n: Int) {
        repeat(n.coerceAtMost(bottom - top + 1)) {
            screen.removeAt(bottom)
            screen.add(top, blankLine())
        }
    }

    private fun saveCursor() {
        savedRow = cursorRow
        savedCol = cursorCol
    }

    private fun restoreCursor() {
        cursorRow = savedRow.coerceIn(0, rows - 1)
        cursorCol = savedCol.coerceIn(0, cols - 1)
        pendingWrap = false
    }

    private fun reset() {
        screen = MutableList(rows) { blankLine() }
        style = TermStyle.DEFAULT
        cursorRow = 0
        cursorCol = 0
        top = 0
        bottom = rows - 1
        pendingWrap = false
        cursorVisible = true
    }

    private fun erase(row: Int, from: Int, to: Int) {
        val line = screen[row]
        for (i in from.coerceAtLeast(0) until to.coerceAtMost(cols)) {
            line[i].ch = ' '
            line[i].style = TermStyle(bg = style.bg)
        }
    }

    private fun shiftLeft(n: Int) {
        val line = screen[cursorRow]
        for (i in cursorCol until cols) {
            val src = i + n
            if (src < cols) {
                line[i].ch = line[src].ch
                line[i].style = line[src].style
            } else {
                line[i].ch = ' '
                line[i].style = TermStyle.DEFAULT
            }
        }
    }

    private fun shiftRight(n: Int) {
        val line = screen[cursorRow]
        for (i in cols - 1 downTo cursorCol) {
            val src = i - n
            if (src >= cursorCol) {
                line[i].ch = line[src].ch
                line[i].style = line[src].style
            } else {
                line[i].ch = ' '
                line[i].style = TermStyle.DEFAULT
            }
        }
    }

    private fun csiDispatch(raw: String, final: Char) {
        val private = raw.isNotEmpty() && raw[0] in "?>="
        val body = if (private) raw.substring(1) else raw
        val ps = body.split(';').map { it.toIntOrNull() }
        fun p(i: Int, def: Int = 1): Int = ps.getOrNull(i)?.takeIf { it > 0 } ?: def
        fun p0(i: Int): Int = ps.getOrNull(i) ?: 0
        if (final != 'm') pendingWrap = false
        when (final) {
            'm' -> if (!private) sgr(ps)
            'A' -> cursorRow = (cursorRow - p(0)).coerceAtLeast(if (cursorRow >= top) top else 0)
            'B', 'e' -> cursorRow = (cursorRow + p(0)).coerceAtMost(if (cursorRow <= bottom) bottom else rows - 1)
            'C', 'a' -> cursorCol = (cursorCol + p(0)).coerceAtMost(cols - 1)
            'D' -> cursorCol = (cursorCol - p(0)).coerceAtLeast(0)
            'E' -> { cursorRow = (cursorRow + p(0)).coerceAtMost(rows - 1); cursorCol = 0 }
            'F' -> { cursorRow = (cursorRow - p(0)).coerceAtLeast(0); cursorCol = 0 }
            'G' -> cursorCol = (p(0) - 1).coerceIn(0, cols - 1)
            'd' -> cursorRow = (p(0) - 1).coerceIn(0, rows - 1)
            'H', 'f' -> {
                cursorRow = (p(0) - 1).coerceIn(0, rows - 1)
                cursorCol = (p(1) - 1).coerceIn(0, cols - 1)
            }
            'J' -> when (p0(0)) {
                0 -> { erase(cursorRow, cursorCol, cols); for (r in cursorRow + 1 until rows) erase(r, 0, cols) }
                1 -> { for (r in 0 until cursorRow) erase(r, 0, cols); erase(cursorRow, 0, cursorCol + 1) }
                2 -> for (r in 0 until rows) erase(r, 0, cols)
                3 -> scrollback.clear()
            }
            'K' -> when (p0(0)) {
                0 -> erase(cursorRow, cursorCol, cols)
                1 -> erase(cursorRow, 0, cursorCol + 1)
                2 -> erase(cursorRow, 0, cols)
            }
            'X' -> erase(cursorRow, cursorCol, cursorCol + p(0))
            'P' -> shiftLeft(p(0).coerceAtMost(cols - cursorCol))
            '@' -> shiftRight(p(0).coerceAtMost(cols - cursorCol))
            'L' -> if (cursorRow in top..bottom) repeat(p(0).coerceAtMost(bottom - cursorRow + 1)) {
                screen.removeAt(bottom)
                screen.add(cursorRow, blankLine())
            }
            'M' -> if (cursorRow in top..bottom) repeat(p(0).coerceAtMost(bottom - cursorRow + 1)) {
                screen.removeAt(cursorRow)
                screen.add(bottom, blankLine())
            }
            'S' -> scrollUp(p(0))
            'T' -> if (!private) scrollDown(p(0))
            'r' -> if (!private) {
                val t = (p(0) - 1).coerceIn(0, rows - 1)
                val b = (p(1, rows) - 1).coerceIn(0, rows - 1)
                if (t < b) { top = t; bottom = b } else { top = 0; bottom = rows - 1 }
                cursorRow = 0
                cursorCol = 0
            }
            's' -> if (!private) saveCursor()
            'u' -> if (!private) restoreCursor()
            'h', 'l' -> if (private) {
                val on = final == 'h'
                for (mode in ps) when (mode) {
                    25 -> cursorVisible = on
                    47, 1047, 1049 -> altScreen(on)
                }
            }
            else -> Unit
        }
    }

    private fun altScreen(on: Boolean) {
        val main = savedMain
        if (on && main == null) {
            saveCursor()
            savedMain = screen
            screen = MutableList(rows) { blankLine() }
        } else if (!on && main != null) {
            screen = main
            savedMain = null
            restoreCursor()
        }
    }

    private fun sgr(ps: List<Int?>) {
        var i = 0
        while (i < ps.size) {
            when (val n = ps[i] ?: 0) {
                0 -> style = TermStyle.DEFAULT
                1 -> style = style.copy(bold = true)
                4 -> style = style.copy(underline = true)
                7 -> style = style.copy(inverse = true)
                22 -> style = style.copy(bold = false)
                24 -> style = style.copy(underline = false)
                27 -> style = style.copy(inverse = false)
                in 30..37 -> style = style.copy(fg = TermPalette.ansi(n - 30))
                39 -> style = style.copy(fg = 0)
                in 40..47 -> style = style.copy(bg = TermPalette.ansi(n - 40))
                49 -> style = style.copy(bg = 0)
                in 90..97 -> style = style.copy(fg = TermPalette.ansi(n - 90 + 8))
                in 100..107 -> style = style.copy(bg = TermPalette.ansi(n - 100 + 8))
                38, 48 -> {
                    var color: Int? = null
                    when (ps.getOrNull(i + 1)) {
                        5 -> {
                            color = ps.getOrNull(i + 2)?.let { TermPalette.xterm(it) }
                            i += 2
                        }
                        2 -> {
                            color = TermPalette.rgb(ps.getOrNull(i + 2) ?: 0, ps.getOrNull(i + 3) ?: 0, ps.getOrNull(i + 4) ?: 0)
                            i += 4
                        }
                    }
                    if (color != null) style = if (n == 38) style.copy(fg = color) else style.copy(bg = color)
                }
                else -> Unit
            }
            i++
        }
    }

    private fun fitWidth(lines: List<Array<Cell>>, c: Int): MutableList<Array<Cell>> =
        lines.map { old -> Array(c) { i -> if (i < old.size) old[i] else Cell() } }.toMutableList()

    /** New grid size; keeps the bottom of the screen (where the prompt is). */
    fun resize(newCols: Int, newRows: Int) {
        val c = newCols.coerceAtLeast(1)
        val r = newRows.coerceAtLeast(1)
        if (c == cols && r == rows) return
        val main = fitWidth(screen, c)
        // Drop blank rows below the cursor first; then push top rows into scrollback.
        while (main.size > r) {
            if (main.size - 1 > cursorRow) {
                main.removeAt(main.size - 1)
            } else {
                val gone = main.removeAt(0)
                if (savedMain == null) scrollback.addLast(gone)
                cursorRow--
            }
        }
        while (main.size < r) main.add(Array(c) { Cell() })
        screen = main
        savedMain?.let { s ->
            val l = fitWidth(s, c)
            while (l.size > r) l.removeAt(0)
            while (l.size < r) l.add(Array(c) { Cell() })
            savedMain = l
        }
        while (scrollback.size > scrollbackMax) scrollback.removeFirst()
        cols = c
        rows = r
        top = 0
        bottom = r - 1
        cursorRow = cursorRow.coerceIn(0, r - 1)
        cursorCol = cursorCol.coerceIn(0, c - 1)
        pendingWrap = false
        version++
    }

    private fun runs(line: Array<Cell>): List<TermRun> {
        var end = line.size
        while (end > 0 && line[end - 1].ch == ' ' && line[end - 1].style.bg == 0 && !line[end - 1].style.inverse) end--
        val out = ArrayList<TermRun>()
        val sb = StringBuilder()
        var cur: TermStyle? = null
        for (i in 0 until end) {
            val cell = line[i]
            if (cur != null && cell.style != cur) {
                out += TermRun(sb.toString(), cur)
                sb.setLength(0)
            }
            cur = cell.style
            sb.append(cell.ch)
        }
        if (cur != null) out += TermRun(sb.toString(), cur)
        return out
    }

    /** Scrollback (oldest first) then the screen rows, as styled runs. */
    fun lines(): List<List<TermRun>> =
        (if (savedMain == null) scrollback.map { runs(it) } else emptyList()) + screen.map { runs(it) }

    /** Plain text of the screen rows (no scrollback), trailing spaces trimmed. */
    fun screenText(): List<String> = screen.map { l -> String(CharArray(l.size) { l[it].ch }).trimEnd() }

    /** Plain text of the scrollback, oldest first. */
    fun scrollbackText(): List<String> = scrollback.map { l -> String(CharArray(l.size) { l[it].ch }).trimEnd() }
}

/** Colours as ARGB (Windows Terminal "Campbell" for the 16 base colours). */
object TermPalette {
    private val ANSI = intArrayOf(
        0x0C0C0C, 0xC50F1F, 0x13A10E, 0xC19C00, 0x0037DA, 0x881798, 0x3A96DD, 0xCCCCCC,
        0x767676, 0xE74856, 0x16C60C, 0xF9F1A5, 0x3B78FF, 0xB4009E, 0x61D6D6, 0xF2F2F2,
    )

    fun rgb(r: Int, g: Int, b: Int): Int =
        (0xFF shl 24) or (r.coerceIn(0, 255) shl 16) or (g.coerceIn(0, 255) shl 8) or b.coerceIn(0, 255)

    fun ansi(i: Int): Int = (0xFF shl 24) or ANSI[i.coerceIn(0, 15)]

    /** xterm 256-colour index. */
    fun xterm(n: Int): Int {
        val i = n.coerceIn(0, 255)
        if (i < 16) return ansi(i)
        if (i >= 232) {
            val v = 8 + (i - 232) * 10
            return rgb(v, v, v)
        }
        val k = i - 16
        val steps = intArrayOf(0, 95, 135, 175, 215, 255)
        return rgb(steps[k / 36], steps[(k / 6) % 6], steps[k % 6])
    }
}
