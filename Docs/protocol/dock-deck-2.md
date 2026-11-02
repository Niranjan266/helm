# Dock & deck 2 (feature `g5`)

Extends PROTOCOL.md §9.3 (dock/deck) and §11 (profiles). The agent adds `"g5"` to
`hello.features`. Everything here is optional for phones: a Phase 1 phone keeps sending and
receiving the exact §9.3 shapes, and a G5 phone talking to an older agent falls back to them.

## 1. Dock folders

Tile kind `"folder"`: `{id, kind:"folder", label?, icon?, children:[tile…]}`.

- `children`: 1–32 `app`/`open` tiles. Folders never nest. Tile ids are unique across the
  whole dock, children included.
- Any tile may carry `icon` (see §4); missing = automatic.
- `dock.get{v:2}` → `{pages, v:2}` with folders as stored.
- `dock.get{}` (Phase 1) → `{pages}` with every folder **replaced by its children, in place**.
- `dock.set{pages}` accepts folder tiles. An old phone's save (made from the flattened view)
  simply dissolves folders; no tile is lost.
- `dock.changed{pages}` always carries the flattened view; G5 phones re-fetch with `v:2`.
- `dock.json` is written as `version:2` when it holds folders or icons; version 1 files read
  unchanged.

Phone UI: long-press and drag a tile onto another to make a folder (dropping on a folder
adds to it; dropping a folder on a folder merges them). Tap a folder to open it as a sheet;
long-press inside to rename, take a tile out, or ungroup.

## 2. Usage stats and smart order

The agent counts, per `app_id`:

| field | counted when |
|---|---|
| `launches` | `apps.launch` succeeds, or a deck `launch` action runs (dock taps use `apps.launch`) |
| `focus` | the foreground watcher (§11) sees a different app come to the front (window → `app_id` via `windows.list`, else the app whose target is the exe) |
| `last_used_ms` | unix ms of the latest of either |

Stored in `usage.json` (`{version:1, apps:{app_id:{launches,focus,last_used_ms}}}`), at most
2000 apps (lowest ranked dropped).

| method | tier | params → result |
|---|---|---|
| `apps.usage` | viewer | `{limit?}` (default 200, max 2000) → `{apps:[{app_id,launches,focus,last_used_ms,score}]}`, most used first |

`score = (3·launches + focus) · 0.5^(days since last use / 14)`; ties by `last_used_ms`
(newer first), then `app_id`. The phone's **Smart order** setting (off by default) sorts each
dock page, folder contents and the app list by `score` (case-insensitive `app_id`, stable
for equal scores; links score 0, a folder scores as its best tile). Saved order is untouched.

## 3. Deck pages and button sizes

- Button gains `size`: `"1x1"` (default, omitted), `"2x1"` (two columns) or `"2x2"`.
- Page: `{id, name, buttons:[button_id…]}` (≤ 32 pages, unique ids ≤ 128 bytes, name ≤ 64).
- `deck.get{v:2}` → `{buttons, pages, v:2}`. Pages are normalized: unknown or repeated ids
  are dropped, buttons on no page go to the end of the first page, and there is always one
  page — a Phase 1 deck reads as `[{id:"main",name:"Main",buttons:[all]}]`.
- `deck.get{}` (Phase 1) → `{buttons}` (all pages' buttons; unknown `size` ignored by old phones).
- `deck.set{buttons, pages}` (G5) replaces both. `deck.set{buttons}` (Phase 1) keeps the stored
  pages and, for buttons sent without `size`, their stored size; new buttons land on page 1.
- `deck.changed{buttons}` unchanged; G5 phones re-fetch with `v:2`.
- `deck.json` becomes `version:2` once it has pages or sizes.
- Phone grid: buttons are packed first-fit, row by row, in page order (a button wider than
  the grid shrinks to its width).

Per-profile default page: a profile's existing `deck_id` (§11) names a deck page — its `id`,
or failing that its `name` (case-insensitive). When `profile.active{id}` arrives, the phone
switches the deck to that page.

## 4. Icons

`icon` on deck buttons and dock tiles (≤ 64 bytes):

| value | meaning |
|---|---|
| missing | automatic: `launch` / app tiles use the real app icon (`apps.icon`); `open` uses the icon of an app whose `app_id` equals the target, else built-in `globe` (URL), `mail` (mailto), `folder` or `file`; media, volume, hotkey, text, macro and HA actions get a matching built-in |
| `b:<name>` | built-in stroke icon drawn by the phone: `play pause next prev volume_up volume_down mute mic mic_off power lock globe folder file terminal gear star heart home mail camera music keyboard monitor bulb code chat game bolt text` |
| `c:<32 hex>` | custom picture uploaded from the phone |
| anything else | Phase 1 free text, shown as text |

| method | tier | params → result |
|---|---|---|
| `deck.icon.put` | remote | `{png:"<base64>"}` (PNG or JPEG, ≤ 256 KiB) → `{icon:"c:<first 32 hex of sha256>"}` |
| `deck.icon.get` | viewer | `{icon}` → `{png:"<base64>"}`; `bad_params` if malformed or missing |

Stored as `deck-icons\<hash>.img` (content-addressed, at most 512). The phone center-crops
and scales pictures to 128×128 PNG before upload and caches fetched icons on disk.
