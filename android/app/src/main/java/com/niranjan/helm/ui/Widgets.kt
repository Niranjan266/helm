package com.niranjan.helm.ui

import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.selection.selectable
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.Slider
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathBuilder
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.niranjan.helm.core.AppEntry

/** An app icon from the PC (cached), or the first letter while loading / if none. */
@Composable
fun AppIcon(appId: String, name: String, icons: IconLoader, size: Dp) {
    val bmp by produceState<Bitmap?>(icons.peek(appId), appId) {
        if (value == null) value = icons.load(appId)
    }
    val b = bmp
    if (b != null) {
        Image(b.asImageBitmap(), contentDescription = null, modifier = Modifier.size(size))
    } else {
        LetterIcon(name, size)
    }
}

@Composable
fun LetterIcon(name: String, size: Dp) {
    val c = Bp.c
    Box(
        Modifier
            .size(size)
            .background(c.fillBlue)
            .border(1.5.dp, c.ink),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            name.trim().firstOrNull()?.uppercase() ?: "?",
            color = Color.White,
            style = if (size >= 40.dp) MaterialTheme.typography.titleLarge else MaterialTheme.typography.titleSmall,
        )
    }
}

/** A titled panel; drawn as a blueprint FIG. box. */
@Composable
fun SectionCard(title: String, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    FigBox(title = title, modifier = modifier.fillMaxWidth()) { content() }
}

@Composable
fun HintText(text: String, modifier: Modifier = Modifier) {
    Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = modifier)
}

/**
 * Searchable list of the PC's apps (`apps.list`). With [allowLink], a second tab adds a
 * URL / path tile instead.
 */
@Composable
fun AppPickerDialog(
    title: String,
    apps: List<AppEntry>?,
    loading: Boolean,
    icons: IconLoader,
    onLoad: () -> Unit,
    onPick: (AppEntry) -> Unit,
    onDismiss: () -> Unit,
    allowLink: Boolean = false,
    onLink: (target: String, label: String) -> Unit = { _, _ -> },
) {
    LaunchedEffect(Unit) { onLoad() }
    var query by remember { mutableStateOf("") }
    var linkMode by remember { mutableStateOf(false) }
    var target by remember { mutableStateOf("") }
    var label by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column {
                if (allowLink) {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        BpFilterChip(selected = !linkMode, onClick = { linkMode = false }, label = { Text("App") })
                        BpFilterChip(selected = linkMode, onClick = { linkMode = true }, label = { Text("Link or path") })
                    }
                    Spacer(Modifier.height(8.dp))
                }
                if (linkMode) {
                    OutlinedTextField(target, { target = it }, label = { Text("URL, folder or file on the PC") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    Spacer(Modifier.height(8.dp))
                    OutlinedTextField(label, { label = it }, label = { Text("Label (optional)") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                } else {
                    OutlinedTextField(query, { query = it }, placeholder = { Text("Search apps") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    Spacer(Modifier.height(8.dp))
                    when {
                        apps == null && loading -> Row(verticalAlignment = Alignment.CenterVertically) {
                            CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                            Spacer(Modifier.width(12.dp))
                            Text("Loading apps…")
                        }
                        apps == null -> HintText("Could not load the app list.")
                        else -> {
                            val q = query.trim().lowercase()
                            val shown = if (q.isEmpty()) apps else apps.filter { q in it.name.lowercase() }
                            LazyColumn(Modifier.heightIn(max = 360.dp)) {
                                items(shown, key = { it.appId }) { app ->
                                    Row(
                                        Modifier.fillMaxWidth().clickable { onPick(app) }.padding(vertical = 8.dp),
                                        verticalAlignment = Alignment.CenterVertically,
                                    ) {
                                        AppIcon(app.appId, app.name, icons, 32.dp)
                                        Spacer(Modifier.width(12.dp))
                                        Text(app.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            if (linkMode) {
                TextButton(enabled = target.isNotBlank(), onClick = { onLink(target, label) }) { Text("Add") }
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Close") } },
    )
}

// ---- Blueprint components -----------------------------------------------------------------

/** "SHEET 01 · YOUR PCS ……… STATUS" strip with a hairline under it. */
@Composable
fun SheetStrip(left: String, modifier: Modifier = Modifier, right: String? = null, rightColor: Color = Color.Unspecified) {
    val c = Bp.c
    Column(modifier.fillMaxWidth()) {
        Row(Modifier.fillMaxWidth().padding(bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(left.uppercase(), style = BpType.mono, color = c.ink, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (right != null) {
                Text(right.uppercase(), style = BpType.mono, color = if (rightColor == Color.Unspecified) c.pencilInk else rightColor, maxLines = 1)
            }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(c.line))
    }
}

/** Mono, uppercase label (status lines, captions). */
@Composable
fun MonoLabel(text: String, modifier: Modifier = Modifier, color: Color = Color.Unspecified, strong: Boolean = false, maxLines: Int = Int.MAX_VALUE) {
    Text(
        text.uppercase(),
        style = if (strong) BpType.monoStrong else BpType.mono,
        color = if (color == Color.Unspecified) Bp.c.ink2 else color,
        modifier = modifier,
        maxLines = maxLines,
        overflow = TextOverflow.Ellipsis,
    )
}

/** Heavy uppercase Archivo heading. */
@Composable
fun DisplayText(text: String, modifier: Modifier = Modifier, style: TextStyle = BpType.title, color: Color = Color.Unspecified, maxLines: Int = 2) {
    Text(
        text.uppercase(),
        style = style,
        color = if (color == Color.Unspecified) Bp.c.ink else color,
        modifier = modifier,
        maxLines = maxLines,
        overflow = TextOverflow.Ellipsis,
    )
}

/**
 * A bordered drawing panel: "FIG. n · title" header strip (with an optional right-hand note)
 * over a padded body. [dashed] draws the outline dashed (things not here yet).
 */
@Composable
fun FigBox(
    modifier: Modifier = Modifier,
    fig: String? = null,
    title: String? = null,
    trailing: String? = null,
    trailingColor: Color = Color.Unspecified,
    dashed: Boolean = false,
    contentPadding: PaddingValues = PaddingValues(horizontal = 12.dp, vertical = 14.dp),
    onTrailingClick: (() -> Unit)? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val c = Bp.c
    Column(
        modifier
            .background(c.panel)
            .then(if (dashed) Modifier.dashedBorder(1.5.dp, c.ink.copy(alpha = 0.6f)) else Modifier.border(1.5.dp, c.ink)),
    ) {
        val head = listOfNotNull(fig?.let { "Fig. $it" }, title).joinToString(" · ")
        if (head.isNotEmpty() || trailing != null) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 7.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(head.uppercase(), style = BpType.mono, color = c.ink, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (trailing != null) {
                    val tm = Modifier.padding(start = 8.dp)
                    Text(
                        trailing.uppercase(),
                        style = BpType.mono,
                        color = if (trailingColor == Color.Unspecified) c.ink2 else trailingColor,
                        maxLines = 1,
                        modifier = if (onTrailingClick != null) Modifier.clickable(onClick = onTrailingClick).then(tm) else tm,
                    )
                }
            }
            Box(Modifier.fillMaxWidth().height(1.dp).background(c.ink.copy(alpha = 0.5f)))
        }
        Column(Modifier.fillMaxWidth().padding(contentPadding), verticalArrangement = Arrangement.spacedBy(10.dp), content = content)
    }
}

/** A dashed rectangular outline. */
fun Modifier.dashedBorder(width: Dp, color: Color): Modifier = drawBehind {
    val w = width.toPx()
    drawRect(
        color,
        topLeft = Offset(w / 2, w / 2),
        size = Size(size.width - w, size.height - w),
        style = Stroke(width = w, pathEffect = PathEffect.dashPathEffect(floatArrayOf(6.dp.toPx(), 4.dp.toPx()))),
    )
}

/** The primary action: pencil-yellow block with a hard offset shadow; it "presses" into it. */
@Composable
fun PencilButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    shadow: Dp = 3.dp,
    contentPadding: PaddingValues = PaddingValues(horizontal = 16.dp),
    textStyle: TextStyle = BpType.button,
    content: @Composable RowScope.() -> Unit,
) {
    val c = Bp.c
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val push = if (pressed && enabled) shadow else 0.dp
    Box(
        modifier
            .defaultMinSize(minWidth = 48.dp, minHeight = 44.dp)
            .padding(end = shadow, bottom = shadow)
            .drawBehind {
                if (enabled && !pressed) {
                    val s = shadow.toPx()
                    drawRect(c.deep, topLeft = Offset(s, s), size = size)
                }
            }
            .offset(x = push, y = push)
            .alpha(if (enabled) 1f else 0.45f)
            .background(c.pencil)
            .border(2.dp, c.deep)
            .clickable(interactionSource = source, indication = null, enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(contentPadding),
        contentAlignment = Alignment.Center,
    ) {
        CompositionLocalProvider(LocalContentColor provides c.deep, LocalTextStyle provides textStyle.copy(color = c.deep)) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally), verticalAlignment = Alignment.CenterVertically, content = content)
        }
    }
}

/** Secondary action: an ink outline on the paper. [danger] = red (asks first). */
@Composable
fun OutlineButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    danger: Boolean = false,
    filled: Boolean = false,
    contentPadding: PaddingValues = PaddingValues(horizontal = 14.dp),
    textStyle: TextStyle = BpType.button,
    content: @Composable RowScope.() -> Unit,
) {
    val c = Bp.c
    val fill = when {
        danger -> c.red.copy(alpha = 0.12f)
        filled -> c.panel
        else -> Color.Transparent
    }
    val ink = if (danger) c.redInk else c.ink
    Box(
        modifier
            .defaultMinSize(minWidth = 44.dp, minHeight = 44.dp)
            .alpha(if (enabled) 1f else 0.45f)
            .background(fill)
            .border(if (danger) 2.dp else 1.5.dp, if (danger) c.red else c.ink)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(contentPadding),
        contentAlignment = Alignment.Center,
    ) {
        CompositionLocalProvider(LocalContentColor provides ink, LocalTextStyle provides textStyle.copy(color = ink)) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally), verticalAlignment = Alignment.CenterVertically, content = content)
        }
    }
}

/** Square icon button with an ink outline (back, menu). */
@Composable
fun SquareIconButton(onClick: () -> Unit, contentDescription: String, modifier: Modifier = Modifier, icon: @Composable () -> Unit) {
    val c = Bp.c
    Box(
        modifier
            .size(44.dp)
            .border(1.5.dp, c.ink)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { this.contentDescription = contentDescription },
        contentAlignment = Alignment.Center,
    ) {
        CompositionLocalProvider(LocalContentColor provides c.ink) { icon() }
    }
}

/** Small mono key / toggle: pencil when [selected], ink outline otherwise, dashed for "more". */
@Composable
fun MonoChip(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    selected: Boolean = false,
    dashed: Boolean = false,
    enabled: Boolean = true,
    height: Dp = 40.dp,
) {
    val c = Bp.c
    val base = Modifier
        .height(height)
        .defaultMinSize(minWidth = 50.dp)
        .alpha(if (enabled) 1f else 0.45f)
    val drawn = when {
        selected -> base.background(c.pencil).border(2.dp, c.deep)
        dashed -> base.dashedBorder(1.5.dp, c.ink.copy(alpha = 0.7f))
        else -> base.border(1.5.dp, c.ink.copy(alpha = 0.7f))
    }
    Box(
        modifier
            .then(drawn)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 8.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text.uppercase(),
            style = if (selected) BpType.monoStrong.copy(fontWeight = FontWeight.Bold) else BpType.monoStrong,
            color = when {
                selected -> c.deep
                dashed -> c.pencilInk
                else -> c.ink
            },
            maxLines = 1,
        )
    }
}

/** A thin rule slider with a rotated-square (diamond) thumb. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DiamondSlider(
    value: Float,
    onValueChange: (Float) -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    valueRange: ClosedFloatingPointRange<Float> = 0f..1f,
    onValueChangeFinished: (() -> Unit)? = null,
    thumbColor: Color = Color.Unspecified,
) {
    val c = Bp.c
    val fill = if (thumbColor == Color.Unspecified) c.pencil else thumbColor
    val source = remember { MutableInteractionSource() }
    Slider(
        value = value,
        onValueChange = onValueChange,
        modifier = modifier.height(32.dp).alpha(if (enabled) 1f else 0.45f),
        enabled = enabled,
        valueRange = valueRange,
        onValueChangeFinished = onValueChangeFinished,
        interactionSource = source,
        thumb = {
            Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
                Box(
                    Modifier
                        .size(15.dp)
                        .rotate(45f)
                        .background(fill)
                        .border(2.dp, c.deep),
                )
            }
        },
        track = { state ->
            val span = (state.valueRange.endInclusive - state.valueRange.start).takeIf { it > 0f } ?: 1f
            val frac = ((state.value - state.valueRange.start) / span).coerceIn(0f, 1f)
            Box(Modifier.fillMaxWidth().height(2.dp).background(c.ink.copy(alpha = 0.35f))) {
                Box(Modifier.fillMaxWidth(frac).height(2.dp).background(fill))
            }
        },
    )
}

/** Comic speech bubble (the app talking to you). */
@Composable
fun Speech(text: String, modifier: Modifier = Modifier) {
    val c = Bp.c
    val shape = RoundedCornerShape(topStart = 14.dp, topEnd = 14.dp, bottomEnd = 14.dp, bottomStart = 2.dp)
    Text(
        text,
        style = BpType.bubble,
        color = c.bubbleInk,
        modifier = modifier
            .background(c.bubble, shape)
            .border(2.dp, c.bubbleInk, shape)
            .padding(horizontal = 12.dp, vertical = 8.dp),
    )
}

/** The Helm mark: an H whose crossbar is a steering hub (same geometry as the website logo). */
val HelmMarkVector: ImageVector by lazy {
    val ink = SolidColor(Color.Black)
    fun dot(cx: Float, cy: Float, r: Float) = PathBuilder().apply {
        moveTo(cx + r, cy)
        arcTo(r, r, 0f, true, true, cx - r, cy)
        arcTo(r, r, 0f, true, true, cx + r, cy)
        close()
    }.nodes
    ImageVector.Builder("HelmMark", 48.dp, 48.dp, 48f, 48f).apply {
        // Stems: 7 x 36 rounded bars.
        for (x in listOf(6f, 35f)) {
            addPath(
                PathBuilder().apply {
                    moveTo(x, 9.5f)
                    arcTo(3.5f, 3.5f, 0f, false, true, x + 7f, 9.5f)
                    lineTo(x + 7f, 38.5f)
                    arcTo(3.5f, 3.5f, 0f, false, true, x, 38.5f)
                    close()
                }.nodes,
                fill = ink,
            )
        }
        // Hub ring and its four spokes.
        addPath(dot(24f, 24f, 7.5f), stroke = ink, strokeLineWidth = 3f)
        addPath(
            PathBuilder().apply {
                moveTo(13f, 24f); lineTo(16.5f, 24f)
                moveTo(31.5f, 24f); lineTo(35f, 24f)
                moveTo(24f, 13f); lineTo(24f, 16.5f)
                moveTo(24f, 31.5f); lineTo(24f, 35f)
            }.nodes,
            stroke = ink,
            strokeLineWidth = 3f,
            strokeLineCap = StrokeCap.Round,
        )
        // Centre and the two knobs.
        addPath(dot(24f, 24f, 2.2f), fill = ink)
        addPath(dot(24f, 11f, 2.2f), fill = ink)
        addPath(dot(24f, 37f, 2.2f), fill = ink)
    }.build()
}

@Composable
fun HelmMark(modifier: Modifier = Modifier, size: Dp = 28.dp, tint: Color = Color.Unspecified) {
    Icon(HelmMarkVector, contentDescription = null, tint = if (tint == Color.Unspecified) Bp.c.pencil else tint, modifier = modifier.size(size))
}

/** "● TEXT" status in mono: green when ok, pencil while reconnecting, ink-2 otherwise. */
@Composable
fun StatusLine(text: String, modifier: Modifier = Modifier, color: Color = Color.Unspecified) {
    MonoLabel("● $text", modifier = modifier, color = if (color == Color.Unspecified) Bp.c.ok else color, maxLines = 1)
}

/** A plain bordered panel (no FIG. header): tiles and list cards. */
@Composable
fun BpCard(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    val c = Bp.c
    Column(modifier.background(c.panel).border(1.5.dp, c.ink), content = content)
}

/** Drop-in for Material's FilterChip in the blueprint style (a [MonoChip] with a label slot). */
@Composable
fun BpFilterChip(
    selected: Boolean,
    onClick: () -> Unit,
    label: @Composable () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
) {
    val c = Bp.c
    val base = Modifier
        .height(36.dp)
        .defaultMinSize(minWidth = 44.dp)
        .alpha(if (enabled) 1f else 0.45f)
    Box(
        modifier
            .then(if (selected) base.background(c.pencil).border(2.dp, c.deep) else base.border(1.5.dp, c.ink.copy(alpha = 0.7f)))
            .selectable(selected = selected, enabled = enabled, role = Role.Checkbox, onClick = onClick)
            .padding(horizontal = 10.dp),
        contentAlignment = Alignment.Center,
    ) {
        val ink = if (selected) c.deep else c.ink
        CompositionLocalProvider(LocalContentColor provides ink, LocalTextStyle provides BpType.monoStrong.copy(color = ink)) { label() }
    }
}
