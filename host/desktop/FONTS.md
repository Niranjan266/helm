# Bundled fonts

The desktop UI ships its fonts in `ui/fonts/` so it never loads anything from the network.
All four families are licensed under the SIL Open Font License 1.1; each license text sits next
to its font (`OFL-*.txt`).

Downloaded unchanged from the official Google Fonts repository,
<https://github.com/google/fonts> (branch `main`, commit `6c1dbe27dcf7`, 2026-11-01):

| File | Source path in google/fonts | Used for |
|---|---|---|
| `Archivo-Variable.ttf` | `ofl/archivo/Archivo[wdth,wght].ttf` | headings, logo word, primary buttons |
| `IBMPlexSans-Variable.ttf` | `ofl/ibmplexsans/IBMPlexSans[wdth,wght].ttf` | body text |
| `IBMPlexMono-Regular.ttf`, `-Medium.ttf`, `-SemiBold.ttf` | `ofl/ibmplexmono/` | labels, tables, log |
| `ComicNeue-Bold.ttf` | `ofl/comicneue/ComicNeue-Bold.ttf` | the yellow speech-bubble line |
| `OFL-*.txt` | `ofl/<family>/OFL.txt` | licenses |
