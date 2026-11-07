package com.niranjan.helm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.unit.dp
import com.niranjan.helm.HelmApp
import com.niranjan.helm.core.QuickKey

/**
 * One-tap key combos on the Screen tab (normal and full-screen). Sent through the same
 * [com.niranjan.helm.core.KeyPress] frames as the on-screen keyboard. "Close window" (Alt+F4)
 * asks first. "Hide" turns the row off (Settings > Screen tab brings it back).
 *
 * [toolbar] (full screen): touches restart its auto-hide timer and the dialog holds it open.
 */
@Composable
fun StreamQuickKeys(
    remote: RemoteViewModel,
    haptics: Boolean,
    modifier: Modifier = Modifier,
    toolbar: ToolbarState? = null,
) {
    val haptic = LocalHapticFeedback.current
    val context = LocalContext.current
    var confirm by remember { mutableStateOf<QuickKey?>(null) }

    fun send(k: QuickKey) {
        remote.flushInput()
        remote.sendAll(k.frames())
        if (haptics) haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
    }

    Row(
        modifier
            .fillMaxWidth()
            .background(Bp.c.panel.copy(alpha = if (toolbar != null) 0.86f else 1f))
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = 8.dp, vertical = 2.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        QuickKey.entries.forEach { k ->
            OutlineButton(
                onClick = {
                    toolbar?.touch()
                    if (k.confirm) confirm = k else send(k)
                },
                danger = k.confirm,
                contentPadding = PaddingValues(horizontal = 10.dp),
            ) { Text(k.label) }
        }
        TextButton(onClick = {
            (context.applicationContext as? HelmApp)?.controller?.settings?.update { it.copy(streamQuickKeys = false) }
        }) { Text("Hide") }
    }

    confirm?.let { k ->
        if (toolbar != null) {
            DisposableEffect(Unit) {
                toolbar.holds++
                onDispose { toolbar.holds-- }
            }
        }
        AlertDialog(
            onDismissRequest = { confirm = null },
            title = { Text("Close the window?") },
            text = { Text("Sends Alt+F4 to the PC. Unsaved work in the active window may be lost.") },
            confirmButton = {
                TextButton(onClick = {
                    confirm = null
                    send(k)
                }) { Text(k.label) }
            },
            dismissButton = { TextButton(onClick = { confirm = null }) { Text("Cancel") } },
        )
    }
}
