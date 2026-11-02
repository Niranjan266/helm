package com.niranjan.helm.core

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

private fun obj(text: String) = Json.parseToJsonElement(text) as JsonObject

private fun app(id: String) = DockTile(id, DockTile.KIND_APP, appId = "App.$id")

class DockFolderCodecTest {
    @Test
    fun foldersRoundTrip() {
        val json = obj(
            """{"pages":[[{"id":"a","kind":"app","app_id":"A"},
               {"id":"f","kind":"folder","label":"Work","icon":"b:folder","children":[
                 {"id":"b","kind":"app","app_id":"B"},{"id":"c","kind":"open","target":"https://x.org","icon":"b:globe"}]}]]}""",
        )
        val pages = DockCodec.decode(json)
        val f = pages[0][1]
        assertEquals(DockTile.KIND_FOLDER, f.kind)
        assertEquals(listOf("b", "c"), f.children.map { it.id })
        assertEquals("b:globe", f.children[1].icon)
        assertEquals(json, DockCodec.encode(pages))
    }

    @Test
    fun oldDockJsonStillDecodes() {
        val json = obj("""{"pages":[[{"id":"t1","kind":"app","app_id":"Fake.Notes","label":"Notes"}]]}""")
        val pages = DockCodec.decode(json)
        assertEquals(DockTile("t1", DockTile.KIND_APP, "Fake.Notes", label = "Notes"), pages[0][0])
        // Encoding a Phase 1 dock adds nothing new (no children, no icon).
        assertEquals(json, DockCodec.encode(pages))
    }

    @Test
    fun dropMakesAndGrowsFolders() {
        val pages = listOf(listOf(app("a"), app("b"), app("c")), listOf(app("d")))
        val one = DockFolders.drop(pages, "b", "a", "F", "Work")
        assertEquals(listOf("F", "c"), one[0].map { it.id })
        assertEquals(listOf("a", "b"), one[0][0].children.map { it.id })
        assertEquals("Work", one[0][0].label)
        // Dropping onto a folder adds to it; an emptied page disappears.
        val two = DockFolders.drop(one, "d", "F", "G")
        assertEquals(1, two.size)
        assertEquals(listOf("a", "b", "d"), two[0][0].children.map { it.id })
        // Folder onto folder merges; never nests.
        val other = DockFolders.drop(two, "c", "x-missing", "H")
        assertEquals(two, other)
        val withSecond = listOf(listOf(two[0][0], DockTile("G2", DockTile.KIND_FOLDER, children = listOf(app("e"), app("f")))))
        val merged = DockFolders.drop(withSecond, "G2", "F", "H")
        assertEquals(1, merged[0].size)
        assertEquals(listOf("a", "b", "d", "e", "f"), merged[0][0].children.map { it.id })
        assertTrue(merged[0][0].children.none { it.kind == DockTile.KIND_FOLDER })
        // Same tile: unchanged.
        assertEquals(pages, DockFolders.drop(pages, "a", "a", "F"))
    }

    @Test
    fun takeOutRemoveUngroupAndLeaves() {
        val folder = DockTile("F", DockTile.KIND_FOLDER, label = "W", children = listOf(app("a"), app("b"), app("c")))
        val pages = listOf(listOf(app("x"), folder, app("y")))
        val out = DockFolders.takeOut(pages, "b")
        assertEquals(listOf("x", "F", "b", "y"), out[0].map { it.id })
        // A folder left with one tile becomes that tile.
        val single = DockFolders.takeOut(out, "a")
        assertEquals(listOf("x", "c", "a", "b", "y"), single[0].map { it.id })
        assertEquals(listOf("x", "F", "y"), DockFolders.remove(pages, "b")[0].map { it.id })
        assertEquals(listOf("x", "y"), DockFolders.remove(pages, "F")[0].map { it.id })
        assertEquals(listOf("x", "a", "b", "c", "y"), DockFolders.ungroup(pages, "F")[0].map { it.id })
        assertEquals(listOf("x", "a", "b", "c", "y"), DockFolders.leaves(pages).map { it.id })
        assertEquals("b", DockFolders.find(pages, "b")?.id)
        val renamed = DockFolders.update(pages, "c") { it.copy(label = "C!") }
        assertEquals("C!", DockFolders.find(renamed, "c")?.label)
    }
}

class SmartOrderTest {
    private val usage = AppUsage.listFrom(
        obj(
            """{"apps":[{"app_id":"App.b","launches":5,"focus":2,"last_used_ms":9,"score":17.0},
               {"app_id":"app.c","launches":1,"focus":0,"last_used_ms":8,"score":3.0}]}""",
        ),
    )

    @Test
    fun parsesUsage() {
        assertEquals(AppUsage("App.b", 5, 2, 9, 17.0), usage[0])
        assertEquals(2, usage.size)
    }

    @Test
    fun sortsTilesAndAppsByScore() {
        val scores = SmartOrder.scores(usage)
        val link = DockTile("l", DockTile.KIND_OPEN, target = "https://x")
        val folder = DockTile("F", DockTile.KIND_FOLDER, children = listOf(app("c")))
        val sorted = SmartOrder.sortTiles(listOf(link, app("a"), folder, app("b")), scores)
        // b (17) > folder via c (3, case-insensitive id) > link and a keep their order (0).
        assertEquals(listOf("b", "F", "l", "a"), sorted.map { it.id })
        val apps = listOf(AppEntry("App.a", "Zed", "desktop"), AppEntry("App.c", "Cee", "desktop"), AppEntry("App.b", "Bee", "desktop"), AppEntry("App.d", "Alpha", "desktop"))
        assertEquals(listOf("Bee", "Cee", "Alpha", "Zed"), SmartOrder.sortApps(apps, scores).map { it.name })
    }
}

class DeckPagesCodecTest {
    private fun btn(id: String, size: String = "1x1") = DeckButton(id, id.uppercase(), "#000000", action = DeckAction.Text(id), size = size)

    @Test
    fun oldDeckMigratesToOneMainPage() {
        val old = obj("""{"buttons":[{"id":"a","label":"A","color":"#000000","action":{"text":"x"}},{"id":"b","label":"B","color":"#000000","action":{"text":"y"}}]}""")
        val l = DeckPages.decode(old)
        assertEquals(listOf(DeckPage("main", "Main", listOf("a", "b"))), l.pages)
        assertEquals("1x1", l.buttons[0].size)
        // Phase 1 encode stays the same shape (no size for 1x1).
        assertEquals(old, DeckCodec.encode(l.buttons))
    }

    @Test
    fun pagesAndSizesRoundTrip() {
        val json = obj(
            """{"buttons":[{"id":"a","label":"A","color":"#000000","action":{"text":"a"},"size":"2x1"},
               {"id":"b","label":"B","color":"#000000","icon":"b:play","action":{"text":"b"},"size":"2x2"},
               {"id":"c","label":"C","color":"#000000","action":{"text":"c"}}],
               "pages":[{"id":"p1","name":"Work","buttons":["a","c"]},{"id":"p2","name":"Media","buttons":["b"]}],"v":2}""",
        )
        val l = DeckPages.decode(json)
        assertEquals("2x2", l.buttons[1].size)
        assertEquals(listOf("a", "c"), l.buttonsOn("p1").map { it.id })
        val expected = JsonObject(json - "v")
        assertEquals(expected, DeckPages.encode(l))
        // Unknown sizes read as 1x1.
        assertEquals("1x1", DeckCodec.decode(obj("""{"buttons":[{"id":"x","action":{"text":"x"},"size":"9x9"}]}"""))[0].size)
    }

    @Test
    fun normalizeDropsUnknownAndRepeatsAndPlacesStrays() {
        val buttons = listOf(btn("a"), btn("b"), btn("c"))
        val pages = listOf(DeckPage("p1", "1", listOf("a", "zz", "a")), DeckPage("p2", "2", listOf("b", "a")))
        assertEquals(
            listOf(DeckPage("p1", "1", listOf("a", "c")), DeckPage("p2", "2", listOf("b"))),
            DeckPages.normalize(pages, buttons),
        )
    }

    @Test
    fun pageOps() {
        var l = DeckLayout(listOf(btn("a"), btn("b")), DeckPages.normalize(emptyList(), listOf(btn("a"), btn("b"))))
        l = DeckPages.addPage(l, "p2", "Games")
        l = DeckPages.upsert(l, btn("c"), "p2")
        assertEquals(listOf("c"), l.buttonsOn("p2").map { it.id })
        l = DeckPages.moveTo(l, "a", "p2")
        assertEquals(listOf("c", "a"), l.pages[1].buttons)
        l = DeckPages.shift(l, "a", -1)
        assertEquals(listOf("a", "c"), l.pages[1].buttons)
        l = DeckPages.renamePage(l, "p2", "Play")
        assertEquals("Play", DeckPages.forProfile(l.pages, "p2")?.name)
        assertEquals("p2", DeckPages.forProfile(l.pages, "play")?.id)
        assertNull(DeckPages.forProfile(l.pages, " "))
        l = DeckPages.deletePage(l, "p2")
        assertEquals(1, l.pages.size)
        assertEquals(listOf("b", "a", "c"), l.pages[0].buttons)
        assertEquals(l, DeckPages.deletePage(l, "main")) // the last page stays
        l = DeckPages.delete(l, "a")
        assertEquals(listOf("b", "c"), l.pages[0].buttons)
        assertFalse(l.buttons.any { it.id == "a" })
    }

    @Test
    fun gridPackingSpansCells() {
        val cells = DeckGrid.pack(listOf(ButtonSize.BIG, ButtonSize.ONE, ButtonSize.WIDE, ButtonSize.ONE, ButtonSize.ONE), 3)
        assertEquals(GridCell(0, 0, 2, 2), cells[0])
        assertEquals(GridCell(0, 2, 1, 1), cells[1])
        // 2x1 does not fit next to the 2x2 on row 1 (one column left): row 2.
        assertEquals(GridCell(2, 0, 2, 1), cells[2])
        assertEquals(GridCell(1, 2, 1, 1), cells[3]) // first-fit backfills the hole
        assertEquals(GridCell(2, 2, 1, 1), cells[4])
        assertEquals(3, DeckGrid.rowCount(cells))
        // Wider than the grid: shrinks to fit.
        assertEquals(GridCell(0, 0, 1, 2), DeckGrid.pack(listOf(ButtonSize.BIG), 1)[0])
    }
}

class IconRefTest {
    @Test
    fun parsesReferences() {
        assertEquals(IconRef.Builtin("play"), IconRef.parse("b:play"))
        assertEquals(IconRef.Legacy("b:nope"), IconRef.parse("b:nope"))
        assertEquals(IconRef.Custom("c:0123456789abcdef0123456789abcdef"), IconRef.parse("c:0123456789abcdef0123456789abcdef"))
        assertEquals(IconRef.Legacy("c:xyz"), IconRef.parse("c:xyz"))
        assertEquals(IconRef.Legacy("wave"), IconRef.parse("wave"))
        assertEquals(IconRef.None, IconRef.parse(null))
        assertEquals(IconRef.None, IconRef.parse("  "))
        assertTrue(BUILTIN_ICONS.size >= 20 && BUILTIN_ICONS.toSet().size == BUILTIN_ICONS.size)
    }

    @Test
    fun automaticIconsUseTheRealAppIcon() {
        val apps = listOf(AppEntry("C:\\Tools\\tool.exe", "Tool", "desktop"))
        fun b(a: DeckAction, icon: String? = null) = DeckButton("x", "", "#000000", icon, a)
        assertEquals(IconRef.App("Fake.Notes"), IconRef.forButton(b(DeckAction.Launch("Fake.Notes")), apps))
        assertEquals(IconRef.App("C:\\Tools\\tool.exe"), IconRef.forButton(b(DeckAction.Open("c:\\tools\\TOOL.exe")), apps))
        assertEquals(IconRef.Builtin("globe"), IconRef.forButton(b(DeckAction.Open("https://example.com")), apps))
        assertEquals(IconRef.Builtin("folder"), IconRef.forButton(b(DeckAction.Open("C:\\Users\\me\\Documents")), apps))
        assertEquals(IconRef.Builtin("file"), IconRef.forButton(b(DeckAction.Open("C:\\notes.txt")), apps))
        assertEquals(IconRef.Builtin("next"), IconRef.forButton(b(DeckAction.Media("next")), null))
        // An explicit icon wins.
        assertEquals(IconRef.Builtin("star"), IconRef.forButton(b(DeckAction.Launch("Fake.Notes"), "b:star"), apps))
        assertEquals(IconRef.App("App.a"), IconRef.forTile(app("a"), null))
        assertEquals(IconRef.Builtin("folder"), IconRef.forTile(DockTile("f", DockTile.KIND_FOLDER), null))
    }
}
