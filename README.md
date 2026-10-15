# Nexus

Platformă AI personală pentru Windows, proiectată pentru extindere. Nexus va reuni un asistent personal, un agent de programare și operații controlate pe PC.

**Stack stabilit:** Rust pentru Core/Host/PC Agent și coordonarea executorilor; TypeScript + React pentru dashboard; Tauri pentru aplicația desktop; Three.js pentru vizualizarea interactivă opțională; SQLite pentru datele locale. Python și PowerShell au roluri specializate, controlate.

## Starea curentă

M0 e închis, iar M1 e complet tehnic. În dashboard scrii o notă, vezi exact ce se va scrie și unde, aprobi sau refuzi, iar Nexus creează fișierul și îl verifică prin hash. Sarcinile supraviețuiesc închiderii ferestrei și căderii Host-ului, fără să se dubleze. Butonul „Fă backup" face o copie consistentă. Ea se poate restaura doar într-un profil nou, unde nimic nu se execută automat.

M2 e în lucru. În tabul Chat vorbești cu Gemini (Google AI Studio) prin cheia ta, ținută doar în seiful Windows (Credential Manager). Modelul și persona le alegi în Setări. Când modelul propune o notă, aceasta așteaptă aprobarea ta exact ca în M1. Totul e verificat față de un server Gemini controlat. Apelul real către Google se verifică după ce introduci o cheie în aplicație.

**Planul și starea implementării au o singură sursă:** [PLAN_NEXUS_RO.md](docs/PLAN_NEXUS_RO.md).

## Dezvoltare

Cerințe pe Windows: Rust stable (`x86_64-pc-windows-msvc`), MSVC Build Tools 2022, WebView2, Node.js LTS și pnpm.

```powershell
pnpm install

# Dashboard în mod dezvoltare; Host-ul trebuie construit lângă el,
# ca butonul „Pornește Hostul” să-l găsească.
cargo build -p nexus-host
pnpm --filter @nexus/desktop tauri dev

# Verificări
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm --filter @nexus/desktop build

# Executabile release (fără installer): target\release\desktop.exe și nexus-host.exe
cargo build -p nexus-host --release --locked
pnpm --filter @nexus/desktop tauri build --no-bundle

# Verificare independentă a unui Host pornit
cargo run -p nexus-client --example health

# Fluxul M1 prin fereastra reală, într-un profil temporar (după build-ul release).
# -Bin <dosar> folosește alt dosar cu desktop.exe + nexus-host.exe, dacă
# target\release\nexus-host.exe e blocat de un Host pornit.
powershell -ExecutionPolicy Bypass -File scripts\m1-ui-check.ps1

# Chatul M2 prin fereastra reală, față de serverul Gemini de test (Google nu e apelat):
# Host debug + falsul de test, plus desktop.exe release.
cargo build -p nexus-host --locked
cargo build -p nexus-host --example fake_gemini_server --locked
powershell -ExecutionPolicy Bypass -File scripts\m2-ui-check.ps1

# Restaurarea unui backup într-un profil NOU (offline; nu atinge profilul curent)
target\release\nexus-host.exe restore <dosar-backup> <dosar-profil-nou>
```

Un profil restaurat pornește în pauză: vezi istoricul, dar nimic nu se execută. Ca să-l activezi după ce l-ai verificat, șterge `restored.json` din el.

`NEXUS_PROFILE_DIR` mută profilul (bază de date, note, token, log) într-un alt director, cu propriul pipe. Așa nu se atinge `%LOCALAPPDATA%\Nexus`. Testele și scriptul de mai sus îl folosesc.

Tipurile TypeScript din `packages/contracts` sunt generate din Rust. După o schimbare de contract: `$env:NEXUS_UPDATE_BINDINGS="1"; cargo test -p desktop`. Testul eșuează dacă fișierul generat a rămas în urmă.

`NEXUS_GEMINI_BASE_URL` (doar în build-ul debug al Host-ului) trimite cererile Gemini la un server de test pe `http://127.0.0.1:<port>`. Build-ul release îl ignoră și vorbește doar cu `https://generativelanguage.googleapis.com`.

Datele locale (tokenul de pairing și log-ul Host-ului, `logs\nexus-host.log`) stau în `%LOCALAPPDATA%\Nexus`, niciodată în repo. Cheile API stau în Credential Manager, sub `Nexus/<hash-profil>/…`, nu în fișiere.

## Documente

| Document | Utilizare |
| --- | --- |
| [AGENTS.md](AGENTS.md) | Reguli concise pentru agentul care lucrează în proiect. |
| [Predare](docs/PREDARE_RO.md) | Context, stare verificată, autorizații și primul pas. |
| [Planul activ](docs/PLAN_NEXUS_RO.md) | Etape, dependențe, rezultate și criterii de acceptare. |
| [Arhitectura](docs/ARHITECTURA_NEXUS_RO.md) | Componente, procese, date și decizii tehnice. |
| [Contractele](docs/CONTRACTE_NEXUS_RO.md) | Tipuri, operații, stări, compatibilitate și comportamentul la erori. |
| [Securitate și validare](docs/SECURITATE_SI_VALIDARE_RO.md) | Limite de încredere și probe obligatorii. |
| [Prompt de pornire](docs/PROMPT_START_RO.md) | Text pe care utilizatorul îl poate trimite pentru a autoriza implementarea. |

Pentru prima preluare se citesc AGENTS.md, predarea și planul. Celelalte documente se consultă pentru etapa executată, fără recitirea întregului pachet înaintea fiecărei modificări.

## Primul rezultat urmărit

O aplicație desktop reală comunică cu un Host Rust separat. Utilizatorul cere crearea unei note într-un spațiu Nexus dedicat, vede conținutul propus, aprobă operația, iar Nexus creează și verifică fișierul. Sarcina, aprobarea și rezultatul persistă. Reconectarea dashboardului și repornirea Hostului nu dublează operația.

Acest flux este local și verificabil fără chei API sau servicii plătite. Modelele AI (M2) se conectează peste această fundație, prin aceeași aprobare.

## Domeniu și control

- Numele produsului este **Nexus**; AION nu este o denumire alternativă.
- Produsul pornește cu un utilizator Windows, pregătit pentru extensii.
- Nexus răspunde în limba în care i se vorbește; cererea explicită pentru altă limbă are prioritate.
- Personalitatea, memoria, starea sarcinilor și drepturile sunt responsabilități separate.
- Furnizorul AI și modelul sunt selectabile. Modelul folosit pentru a programa Nexus nu devine implicit furnizorul produsului.
- Citirea documentației nu pornește implementarea, nu instalează software și nu autorizează publicarea.
