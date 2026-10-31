package com.niranjan.helm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AssistChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.Shells
import com.niranjan.helm.core.TermKeys
import com.niranjan.helm.core.TermRun
import com.niranjan.helm.core.TermSize
import com.niranjan.helm.core.TermStyle
import kotlinx.coroutines.launch

private val TERM_BG = Color(0xFF0C0C0C)
private val TERM_FG = Color(0xFFCCCCCC)
private val TERM_TEXT = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 12.sp, lineHeight = 15.sp)

private fun color(argb: Int, default: Color) = if (argb == 0) default else Color(argb)

private fun spanOf(s: TermStyle): SpanStyle {
    var fg = color(s.fg, TERM_FG)
    var bg = color(s.bg, Color.Transparent)
    if (s.inverse) {
        val f = fg
        fg = if (s.bg == 0) TERM_BG else bg
        bg = f
    }
    return SpanStyle(
        color = fg,
        background = bg,
        fontWeight = if (s.bold) FontWeight.Bold else null,
        textDecoration = if (s.underline) TextDecoration.Underline else null,
    )
}

/** One terminal line; [cursorCol] >= 0 draws a block cursor there. */
private fun lineText(runs: List<TermRun>, cursorCol: Int): AnnotatedString = buildAnnotatedString {
    var col = 0
    for (r in runs) {
        if (cursorCol in col until col + r.text.length) {
            val i = cursorCol - col
            withStyle(spanOf(r.style)) { append(r.text.substring(0, i)) }
            withStyle(SpanStyle(color = TERM_BG, background = TERM_FG)) { append(r.text[i]) }
            withStyle(spanOf(r.style)) { append(r.text.substring(i + 1)) }
        } else {
            withStyle(spanOf(r.style)) { append(r.text) }
        }
        col += r.text.length
    }
    if (cursorCol >= col) {
        append(" ".repeat(cursorCol - col))
        withStyle(SpanStyle(color = TERM_BG, background = TERM_FG)) { append(' ') }
    }
}

/** Phase 5 terminal (§13 `term.*`): PowerShell or cmd on the PC. Admin only, biometric check. */
@Composable
fun TerminalScreen(vm: ExtrasViewModel) {
    val ui by vm.ui.collectAsStateWithLifecycle()
    val t = ui.term
    val gate = LocalAdminGate.current
    var shell by rememberSaveable { mutableStateOf(Shells.POWERSHELL) }
    var input by rememberSaveable { mutableStateOf("") }
    var ctrl by remember { mutableStateOf(false) }
    var grid by remember { mutableStateOf(80 to 24) }
    val measurer = rememberTextMeasurer()
    val density = LocalDensity.current
    val list = rememberLazyListState()
    val scope = rememberCoroutineScope()
    val cell = remember(measurer) {
        val r = measurer.measure(AnnotatedString("M".repeat(10)), TERM_TEXT)
        (r.size.width / 10f) to with(density) { TERM_TEXT.lineHeight.toPx() }
    }

    LaunchedEffect(grid, t.termId) { vm.termResize(grid.first, grid.second) }
    LaunchedEffect(t.lines.size, t.cursorRow) {
        if (t.lines.isNotEmpty()) list.scrollToItem((t.screenStart + t.cursorRow).coerceIn(0, t.lines.lastIndex))
    }

    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp, vertical = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Shells.ALL.forEach { s ->
                BpFilterChip(selected = shell == s, onClick = { shell = s }, label = { Text(if (s == Shells.CMD) "cmd" else "PowerShell") })
            }
            PencilButton(enabled = !t.opening, onClick = {
                gate("Open a $shell terminal on the PC") { vm.openTerminal(shell, grid.first, grid.second) }
            }) { Text(if (t.termId == null || t.exited) "Open" else "Restart") }
            if (t.termId != null && !t.exited) OutlineButton(onClick = vm::closeTerminal) { Text("Close") }
            Text("${grid.first}×${grid.second}", style = MaterialTheme.typography.labelSmall)
        }
        Box(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .background(TERM_BG)
                .onSizeChanged { s -> grid = TermSize.fit(s.width.toFloat() - 8f, s.height.toFloat(), cell.first, cell.second) }
                .padding(4.dp),
        ) {
            if (t.termId == null && !t.opening) {
                Text(
                    "Runs a shell on the PC with your Windows account. Needs the admin access level; " +
                        "every session is written to the PC's audit log.",
                    color = TERM_FG, style = MaterialTheme.typography.bodySmall, modifier = Modifier.align(Alignment.Center).padding(16.dp),
                )
            }
            LazyColumn(state = list, modifier = Modifier.fillMaxSize()) {
                items(t.lines.size) { i ->
                    val cur = if (!t.exited && t.cursorVisible && i == t.screenStart + t.cursorRow) t.cursorCol else -1
                    Text(lineText(t.lines[i], cur), style = TERM_TEXT, color = TERM_FG, softWrap = false, maxLines = 1)
                }
            }
            if (t.exited) {
                Text(
                    "Session ended" + (t.exitCode?.let { " (exit $it)" } ?: ""),
                    color = Color(0xFFF9F1A5), style = MaterialTheme.typography.labelMedium,
                    modifier = Modifier.align(Alignment.BottomEnd).padding(4.dp),
                )
            }
        }
        TermKeyRow(ctrl, onCtrl = { ctrl = it }, enabled = t.termId != null && !t.exited) { vm.termSend(it) }
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            OutlinedTextField(
                value = input,
                onValueChange = { v ->
                    // With Ctrl held, the first typed letter becomes a control key.
                    if (ctrl && v.length == input.length + 1) {
                        TermKeys.ctrl(v.last())?.let { vm.termSend(it) }
                        ctrl = false
                    } else {
                        input = v
                    }
                },
                enabled = t.termId != null && !t.exited,
                singleLine = true,
                textStyle = TERM_TEXT.copy(fontSize = 14.sp),
                placeholder = { Text("Command") },
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send, autoCorrect = false),
                keyboardActions = KeyboardActions(onSend = {
                    vm.termSend(input + TermKeys.ENTER)
                    input = ""
                    scope.launch { if (t.lines.isNotEmpty()) list.scrollToItem(t.lines.lastIndex) }
                }),
                modifier = Modifier.weight(1f),
            )
            TextButton(enabled = t.termId != null && !t.exited, onClick = {
                vm.termSend(input + TermKeys.ENTER)
                input = ""
            }) { Text("Run") }
        }
    }
}

@Composable
private fun TermKeyRow(ctrl: Boolean, onCtrl: (Boolean) -> Unit, enabled: Boolean, send: (String) -> Unit) {
    val keys = listOf(
        "Esc" to TermKeys.ESC, "Tab" to TermKeys.TAB, "^C" to "\u0003", "↑" to TermKeys.UP, "↓" to TermKeys.DOWN,
        "←" to TermKeys.LEFT, "→" to TermKeys.RIGHT, "Home" to TermKeys.HOME, "End" to TermKeys.END,
        "PgUp" to TermKeys.PAGE_UP, "PgDn" to TermKeys.PAGE_DOWN, "Del" to TermKeys.DELETE, "⌫" to TermKeys.BACKSPACE,
        "Enter" to TermKeys.ENTER,
    )
    Row(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        BpFilterChip(selected = ctrl, enabled = enabled, onClick = { onCtrl(!ctrl) }, label = { Text("Ctrl") })
        keys.forEach { (label, seq) -> AssistChip(enabled = enabled, onClick = { send(seq) }, label = { Text(label) }) }
    }
}
