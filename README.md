# Nexus

Platformă AI personală pentru Windows, proiectată pentru extindere. Nexus va reuni un asistent personal, un agent de programare și operații controlate pe PC.

**Stack stabilit:** Rust pentru Core/Host/PC Agent și coordonarea executorilor; TypeScript + React pentru dashboard; Tauri pentru aplicația desktop; Three.js pentru vizualizarea interactivă opțională; SQLite pentru datele locale. Python și PowerShell au roluri specializate, controlate.

## Starea curentă

M0 este în lucru: Host-ul Rust separat, IPC pe named pipe și dashboardul Tauri care se conectează la el funcționează și sunt probate. Circuitul complet cu note aprobate (M1) nu există încă.

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
```

Tipurile TypeScript din `packages/contracts` sunt generate din Rust. După o schimbare de contract: `$env:NEXUS_UPDATE_BINDINGS="1"; cargo test -p desktop`. Testul eșuează dacă fișierul generat a rămas în urmă.

Datele locale (tokenul de pairing și log-ul Host-ului, `logs\nexus-host.log`) stau în `%LOCALAPPDATA%\Nexus`, niciodată în repo.

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

Acest flux este local și verificabil fără chei API sau servicii plătite. Conectarea modelelor AI urmează după această fundație.

## Domeniu și control

- Numele produsului este **Nexus**; AION nu este o denumire alternativă.
- Produsul pornește cu un utilizator Windows, pregătit pentru extensii.
- Nexus răspunde în limba în care i se vorbește; cererea explicită pentru altă limbă are prioritate.
- Personalitatea, memoria, starea sarcinilor și drepturile sunt responsabilități separate.
- Furnizorul AI și modelul sunt selectabile. Modelul folosit pentru a programa Nexus nu devine implicit furnizorul produsului.
- Citirea documentației nu pornește implementarea, nu instalează software și nu autorizează publicarea.
