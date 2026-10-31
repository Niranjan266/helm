# Bundled fonts

The Blueprint theme (`app/src/main/java/com/niranjan/helm/ui/Theme.kt`) uses four font families.
All are licensed under the SIL Open Font License 1.1; the full licence texts ship inside the APK
in `app/src/main/assets/font-licenses/`.

| File in `app/src/main/res/font/` | Family | Source (official Google Fonts repo) |
|---|---|---|
| `archivo_variable.ttf` | Archivo (variable wdth/wght; display, 800–900) | `https://github.com/google/fonts/blob/main/ofl/archivo/Archivo[wdth,wght].ttf` — © 2020 The Archivo Project Authors |
| `ibm_plex_sans_variable.ttf` | IBM Plex Sans (variable; body 400–600) | `https://github.com/google/fonts/blob/main/ofl/ibmplexsans/IBMPlexSans[wdth,wght].ttf` — © 2017 IBM Corp. |
| `ibm_plex_mono_regular.ttf`, `_medium.ttf`, `_semibold.ttf` | IBM Plex Mono (labels) | `https://github.com/google/fonts/tree/main/ofl/ibmplexmono` — © 2017 IBM Corp. |
| `comic_neue_bold.ttf` | Comic Neue Bold (speech bubbles) | `https://github.com/google/fonts/blob/main/ofl/comicneue/ComicNeue-Bold.ttf` — © 2014 The Comic Neue Project Authors |

Files were renamed only to satisfy Android resource naming (lowercase, underscores); their
contents are unmodified. "Plex" is a Reserved Font Name: do not ship modified copies under it.
