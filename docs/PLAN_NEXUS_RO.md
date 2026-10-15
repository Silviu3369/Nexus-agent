# Plan activ Nexus

Data de referință: **2026-10-15**, ora Europei Centrale. Acesta este unicul registru al progresului.

## 1. Starea curentă și următoarea acțiune

| Element | Stare verificată |
| --- | --- |
| Cerințe și stack | Stabilite în conversație; documentate în acest pachet. |
| Predare | Pregătită. |
| Implementare | Autorizată explicit de utilizator; un singur agent de cod lucrează la Nexus. Plan de implementare M0+M1 aprobat. |
| Repo Git | Privat pe GitHub (`Silviu3369/Nexus-agent`), branch `main` urmărește `origin/main`. |
| Mediu de dezvoltare | Node.js 24.20.0 + npm 11.19.0 + pnpm 12.11.2 instalate real. Rust/Cargo/MSVC/WebView2 confirmate prin build real, debug și release. |
| Cod de produs | `nexus-protocol` (ID-uri tipate, DTO-uri, erori, serviciu tarpc, pairing, folder de profil); `nexus-host` (named pipe, lock de instanță unică, sesiune autentificată per conexiune, log în `%LOCALAPPDATA%\Nexus\logs`, plafonat la 10 MiB + o copie); `nexus-client` (connect, handshake, reîncercare pe pipe ocupat); `apps/desktop` (comenzile `host_status`/`start_host`, ecranul de stare al Host-ului); `packages/contracts` (tipuri TS generate din Rust). `nexus-core` și `nexus-storage` sunt goale până la M1. |
| Verificări | `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked` (13 teste reale, toate PASS; suita IPC rulată de 5 ori la rând fără nicio cădere) și `pnpm --filter @nexus/desktop build` (inclusiv `tsc`) trec. |
| Dimensiuni măsurate | Build release fără installer: `desktop.exe` 4,6 MB, `nexus-host.exe` 0,65 MB. WebView2 e cel deja prezent în Windows. Installerul și consumul la rulare nu sunt încă măsurate. |
| ADR-IPC-001 | **Închis prin probă** (detalii în arhitectură §13): named pipe Windows + tarpc peste `LengthDelimitedCodec` + JSON, tipuri TS generate cu specta 1.0.5. |
| Matrice M0 | ENV-01 **PASS**: `desktop.exe` release pornit direct, interfața citită și butonul „Pornește Hostul” apăsat prin Windows UI Automation; Host-ul real a răspuns, iar fereastra afișează „Conectat, nexus-host, v1”. IPC-01 **PASS**. IPC-02 **PASS**: token greșit, versiune incompatibilă, mesaj >1 MiB, apel fără handshake, handshake eșuat care revocă sesiunea și „Host fals”. Pentru acesta din urmă, clientul verifică SID-ul procesului care deține pipe-ul înainte să trimită tokenul: testul pe pipe real acceptă serverul sub SID-ul curent și îl refuză față de un SID diferit. Fără port de rețea, fiindcă nu există cod TCP. Varianta cap-coadă, cu un Host pornit efectiv sub alt cont Windows, e **BLOCKED**: cere un al doilea cont pe PC. LIFE-01 **PASS pentru partea M0**: UI închis prin WM_CLOSE → Host-ul trăiește (același PID), clientul independent răspunde, UI-ul redeschis se reconectează la aceeași instanță. Partea „în timpul unei sarcini” cere M1. LIFE-02 **PASS**: testul de integrare plus procese reale (a doua instanță iese cu cod 1 și scrie motivul în log). |
| Probleme deschise | (1) `tauri-plugin-opener` din scaffold e încă activ (`opener:default`), deși nimic nu-l folosește; de restrâns sau eliminat înainte de M1. (2) CSP e `null` în `tauri.conf.json`; trebuie setat înainte de afișarea conținutului din M1 (UI-02). (3) Butonul „Pornește Hostul” e verificat pe release; sub `tauri dev` (Job Object-ul lui cargo) nu e încă verificat. (4) Proba IPC-02 cu un Host sub alt cont Windows e BLOCKED (vezi mai sus). |
| Dependențe adăugate în M0 | Din lotul aprobat: `tokio`, `tarpc`, `tokio-serde`, `tokio-util`, `serde`/`serde_json`, `specta`, `tauri` v2, `tracing`/`tracing-subscriber`, `thiserror`, `uuid`, plus `anyhow` și `futures` ca utilitare. `windows-sys` 0.61.2 a devenit dependență directă a `nexus-client`, pentru verificarea SID-ului; era deja în `Cargo.lock` prin Tokio, deci nu a intrat niciun crate nou. |
| M0–M7 de mai jos | **M0 închis**: toate cazurile M0 sunt PASS, cu excepția variantei cross-account IPC-02, marcată BLOCKED. M1 în lucru. M2–M7 neîncepute. |
| Progres M1 | **Gata și testat**, încă nelegat în Host: (a) `nexus-core`: stările Run/Action/Operation ca enum-uri închise, cu tranzițiile din contracte §4; (b) `nexus-storage`: SQLite cu schema v1 completă pentru M1, WAL, `synchronous=FULL`, chei externe, fir unic care deține conexiunea, refuzul schemelor mai noi, al căilor de rețea și al stărilor necunoscute, evenimente în aceeași tranzacție cu starea; (c) `CreateMarkdownArtifact`: cale derivată doar din OperationId, staging + flush + publicare fără suprascriere + verificare SHA-256, refuzul joncțiunilor reale. **PATH-01 PASS**: textul ostil (traversare, C:\, \\?\, UNC, CON, ADS) rămâne doar conținut, iar o joncțiune reală în locul directorului de proiect sau de staging e refuzată fără să se scrie ceva prin ea. (d) depozitul `nexus-storage`: fiecare operație de domeniu e o singură tranzacție, cu stare plus evenimente. Logica de stare e dovedită pe SQLite real pentru RUN-01 (ciclul complet), RUN-02 (refuz/expirare), RUN-03 (idempotency și conflict), RUN-04 (run întrerupt → reconciliere → aceeași OperationId, attempt 2), RUN-07 (anulare în coadă/așteptare/execuție), SEC-01 (manifest schimbat, replay, consum dublu) și EVT-01 (cursor și watermark). Aceste cazuri sunt PASS **la nivel de tranzacții**; probele cap-coadă prin Host și UI, plus RUN-05/06 cu procese reale oprite, sunt încă NOT RUN. |
| Dependențe noi în M1 | `rusqlite` 0.40.2 cu `bundled` (din lotul aprobat), care aduce tranzitiv 8 pachete: `libsqlite3-sys`, `hashlink`, `hashbrown`, `fallible-iterator`, `fallible-streaming-iterator`, `vcpkg`, plus `sqlite-wasm-rs`/`rsqlite-vfs`, compilate doar pentru WebAssembly. `sha2` 0.10.9 a devenit dependență directă; era deja în `Cargo.lock` prin Tauri. |
| Acțiune următoare | Runtime-ul M1 în Host: depozitul pentru task/run/action/approval, comenzile `SubmitTask`/`GetRun`/`DecideApproval`/`CancelRun`, executorul serial și reconcilierea la pornire; apoi UI-ul și probele RUN-01…07. |

Cererea curentă autorizează implementarea M0+M1 conform planului aprobat (pașii din secțiunile 4 și 5 de mai jos). [Promptul de pornire](PROMPT_START_RO.md) rămâne valabil ca text de referință, nu mai e singurul declanșator.

La fiecare predare se actualizează acest tabel și jurnalul de la final cu dovezi actuale. Starea mediului din predare este o observație datată, nu o garanție permanentă.

## 2. Rezultatul produsului

Nexus este un agent personal Windows, cu dashboard, furnizor/model selectabil, personalitate editabilă și răspunsuri în limba utilizatorului. Rolurile sunt asistență generală, programare și operații controlate pe PC.

Fundația comună trebuie să susțină memoria pe proiect, sarcini de durată, aprobări, automatizări și integrări. Limitele de acces și recuperarea sunt implementate înaintea execuției generale de cod.

## 3. Ordinea etapelor

| Etapă | Rezultat | Dependențe | Stare |
| --- | --- | --- | --- |
| M0 | Mediu verificat, workspace și transport local demonstrat | Pornirea explicită a implementării; aprobările aplicabile | Neînceput |
| M1 | Circuit local complet, persistent și recuperabil | M0 | Neînceput |
| M2 | Conversație AI reală, modele selectabile, personalitate | M1 | Neînceput |
| M3 | Agent de programare, PC și browser cu izolare verificată | M1; M2 pentru planificarea prin AI | Neînceput |
| M4 | Automatizări durabile și Telegram | M1, M2; instrumentele folosite din M3 | Neînceput |
| M5 | Memorie, documente și căutare cu proveniență | M2; executor pentru importuri neîncrezătoare din M3 | Neînceput |
| M6 | Distribuție Windows calificată pentru utilizare zilnică | Funcțiile selectate din M1–M5 și toate probele lor | Neînceput |
| M7 | Extensii avansate, voce, Three.js, alte dispozitive | Contracte și limite validate în etapele precedente | Neînceput |

Nu sunt estimate termene sau dimensiuni de instalare fără builduri măsurate. O etapă se închide prin rezultate verificabile, nu prin numărul de fișiere.

Scopul ferm pe termen scurt este M0–M2; M3–M7 rămân roadmap, re-prioritizabil pe baza dovezilor din M1/M2, nu scop angajat dinainte. Aceasta nu redeschide deciziile fixate (secțiunea 7 din predare); schimbă doar ordinea/întinderea muncii viitoare, nu arhitectura aleasă.

## 4. M0 — Mediu, workspace și decizii de implementare

1. Recitește predarea și verifică starea actuală a fișierelor, instrumentelor și permisiunilor.
2. Verifică versiunile compatibile din documentația oficială. Pregătește lista exactă de dependențe pentru etapa curentă și rezolvă aprobările necesare o singură dată pentru lot.
3. Creează workspace Cargo și workspace pnpm pentru React/TypeScript + Tauri. Fixează versiunile în manifeste și lockfiles; include reguli pentru excluderea datelor locale, secretelor, buildurilor și probelor.
4. Creează numai componentele necesare pentru M1. Layoutul din arhitectură este direcția, nu o cerință de a genera proiecte goale pentru fiecare modul viitor.
5. Demonstrează un Host separat, o singură instanță per profil, conexiune locală autentificată, negocierea versiunii și reconectarea dashboardului.
6. Închide decizia de transport din arhitectură printr-o probă documentată. Verifică identitatea serverului, identitatea clientului, respingerea unei instanțe false și absența unui port public.
7. Deschide aplicația Tauri reală pe Windows, verifică titlul și o interacțiune care primește răspuns de la Host.
8. Completează comenzile reproductibile de dezvoltare și testare în README.

**Acceptare:** Tauri și Host se construiesc; o comandă tipată traversează IPC; versiunile incompatibile și clienții neautorizați sunt respinși; Hostul supraviețuiește închiderii dashboardului. Conexiunea nu acordă implicit drepturi tuturor operațiilor.

**Puncte de decizie:** transportul și autentificarea, formatul contractelor generate, backendul Windows de izolare pentru M3. Ultimul poate rămâne în evaluare la M0 dacă execuția neîncrezătoare rămâne dezactivată.

## 5. M1 — Primul circuit complet

### Cazul utilizatorului

Utilizatorul deschide Nexus, cere crearea unei note Markdown în spațiul de artefacte Nexus, vede destinația și conținutul exact, aprobă, apoi deschide fișierul rezultat. Operația se înregistrează și poate fi verificată după reconectare sau repornire.

Acesta este un instrument local determinist, fără model AI fals. Nu citește documente personale și nu execută shell.

### Implementare

1. Tipuri pentru proiect, sarcină, run, pas, acțiune, aprobare, artefact și eveniment.
2. SQLite cu schemă versionată, tranzacții, chei externe, WAL pe disc local și evenimente persistente în aceeași tranzacție cu starea.
3. Coadă persistentă; primul executor poate procesa serial. Limite de timp, coadă și dimensiune a cererilor.
4. Politică implicită restrictivă și un instrument explicit: crearea unei note noi în spațiul Nexus dedicat. Fără suprascriere, traversare sau legături care ies din acel spațiu.
5. Aprobare legată de manifestul exact al operației și consumată atomic pentru acea operație. Revocarea drepturilor și schimbarea parametrilor invalidează execuția.
6. Intenție salvată înaintea efectului, verificarea fișierului după creare, înregistrarea hashului și a rezultatului. Cazurile incerte intră la reconciliere.
7. Dashboard cu fluxul principal, activitate, aprobări și rezultat deschizabil. Stări reale pentru încărcare, lipsă conexiune, eroare, anulare și conflict.
8. Pornire, închidere UI, reconectare, oprire explicită Host și recuperare după întrerupere.
9. Backup consistent și restaurare într-un profil de test separat, fără a activa automat programări sau a relua acțiuni externe.
10. Audit structurat și teste cu defecte injectate la punctele critice.

**Acceptare:** toate cazurile obligatorii M0/M1 din [matricea de validare](SECURITATE_SI_VALIDARE_RO.md) au probe. Oprirea Hostului după crearea fișierului, înaintea salvării rezultatului, nu produce un al doilea fișier la revenire. Un efect care nu poate fi demonstrat rămâne marcat incert.

**Livrare pentru utilizator:** aplicația nativă verificată, instrucțiuni de pornire, localizarea artefactului de probă, raport concis al verificărilor și limitelor. Un screenshot nu înlocuiește probele de interacțiune și recuperare.

## 6. M2 — Furnizori AI și personalitate

- Primul adaptor: Gemini, prin Google AI Studio (nivelul gratuit), pentru a porni fără costuri. Al doilea: OpenAI, pentru a verifica un protocol de furnizor diferit. DeepSeek și un endpoint local compatibil urmează prin adaptoare explicite.
- Nivelul gratuit Google AI Studio are limite de rate (RPM/RPD/TPM, variabile pe model) și poate avea o politică de folosire a datelor pentru îmbunătățirea modelului diferită de nivelul plătit/Vertex AI. Se verifică termenii curenți la implementare, înainte de a trimite orice conținut al utilizatorului; alegerea nivelului gratuit este explicită și vizibilă în configurarea conexiunii, nu implicită și ascunsă.
- Catalog de capabilități: streaming, instrumente, imagini, rezultate structurate, limite de context și informații de utilizare. Nu presupune compatibilitate doar fiindcă două API-uri folosesc JSON.
- Conexiuni configurate de utilizator; seif Windows pentru secrete; endpointurile și modelul sunt selectate explicit.
- Context construit din instrucțiuni, personalitate, cerere și istoric autorizat; datele proiectului se includ numai în domeniul permis.
- Conversații persistente, profil de personalitate versionat și regulă de limbă automată. Separă limba răspunsului de limba documentelor citate.
- Bugete de tokeni, timp și număr de pași; cost estimat separat de utilizarea raportată.
- Integrează propunerile de instrumente ale modelului cu același broker din M1. Rezultatul modelului nu poate emite o aprobare.
- Nu efectua apeluri plătite și nu căuta chei în alte proiecte fără autorizare. Fără credențiale aprobate, testele cu server controlat rămân probe de protocol, iar integrarea reală este raportată ca neverificată.

**Acceptare:** selectarea unui model produce cereri prin adaptorul corect; comportamentele de anulare, limită, timeout și răspuns invalid sunt testate; cheile nu apar în UI, prompturi sau loguri. Nu există schimbare tacită de furnizor.

## 7. M3 — Cod, PC și browser

- Adaptor Windows Rust cu operații tipate și drepturi minime; elevare doar pentru operațiile selectate, niciodată pentru întregul produs în mod implicit.
- Codul neîncrezător rulează într-un backend de izolare demonstrat pe Windows. Procesul, directorul de lucru și Job Objects nu sunt suficiente singure.
- Pentru programare: copie de lucru izolată, schimbări inspectabile, verificări potrivite proiectului și aplicarea modificărilor cu verificarea precondițiilor. Păstrează modificările existente ale utilizatorului.
- Pentru browser: profil dedicat, domenii și conturi autorizate, verificare înaintea acțiunilor externe. Citirea paginilor nu autorizează trimiterea mesajelor sau efectuarea tranzacțiilor.
- Python, Node sau PowerShell pot fi instrumente într-un executor; folosirea lor nu mută autoritatea din Host.
- Operațiile cu mouse/tastatură verifică ținta și starea ferestrei; capturile ecranului respectă politica de date.
- Aprobările sensibile folosesc o cale de confirmare de încredere; un renderer care afișează conținut extern nu poate fabrica decizia utilizatorului.
- Demonstrează lipsa accesului neautorizat la fișiere, rețea, secrete, pipe-ul Hostului și procesele Hostului.

**Acceptare:** un flux real de modificare și testare de cod, plus o operație PC verificată, trec prin policy, broker, audit și verificarea rezultatului. Instrumentele cu izolare nedemonstrată rămân indisponibile.

## 8. M4 — Automatizări și Telegram

- Scheduler persistent cu fus orar, schimbări de oră, programări ratate, suprapuneri și revocare.
- Fiecare execuție reevaluează permisiunile și bugetul; aprobarea unei programări are un domeniu explicit.
- Telegram: API oficial, asociere explicită a identității numerice, chat privat inițial, deduplicarea update-urilor și scope restrâns. Long polling este prima opțiune pentru PC-ul personal.
- Acțiunile critice pot solicita aprobare în dashboard; un nume de utilizator sau un mesaj redirecționat nu este dovadă de identitate.
- PC-ul trebuie să fie pornit, conectat și Hostul activ. Nu promite execuție când calculatorul este oprit.
- WhatsApp rămâne condiționat de verificarea eligibilității și regulilor oficiale valabile atunci.

**Acceptare:** update-uri și declanșări duplicate nu dublează efectele; un calendar ratat nu declanșează o rafală de operații; revocarea oprește următoarele acțiuni.

## 9. M5 — Memorie și documente

- Preferințe personale, cunoștințe pe proiect și context temporar separate.
- Fiecare informație memorată are proveniență, dată și politică de păstrare; se poate inspecta, corecta, exporta și șterge.
- Căutare textuală inițială; embeddings numai când îmbunătățirea este demonstrată și politica de date permite. Dacă devin necesare, se preferă API-ul de embeddings al unui furnizor deja conectat (ex: Gemini) înaintea unor greutăți de model local, pentru amprentă mică pe disc.
- Indexurile sunt derivate și reconstruibile. Schimbarea modelului de embeddings nu amestecă vectori incompatibili.
- Importul documentelor aplică limite de mărime, expandare, tip și timp, plus izolare pentru parserele care o cer.
- Personalitatea nu se rescrie din documente sau rezultate de instrumente.

**Acceptare:** memoria unui proiect nu apare neautorizat în altul; ștergerea propagă în indexuri și respectă politica documentată a backupurilor; răspunsurile pot indica sursele relevante.

## 10. M6 — Calificare pentru utilizare zilnică

- Profiluri per utilizator, fișiere protejate, backup/restaurare și export coerent. Modelul de criptare la repaus și de recuperare a cheilor este decis și verificat.
- Pachet Windows, verificarea integrității și strategia de semnare; publicarea și certificatul necesită autorizațiile aplicabile.
- Probe pe instalare curată, upgrade cu date existente, incompatibilitate de versiuni, revenire după eșec, dezinstalare și păstrarea/ștergerea datelor conform alegerii utilizatorului.
- Actualizarea coordonează Hostul, UI și schema; nu înlocuiește executori în mijlocul operațiilor.
- Audit de dependențe și secrete, licențe, accesibilitate, consum măsurat și diagnostic exportabil cu date sensibile eliminate.
- Selectează explicit funcțiile lansate; funcțiile încă neverificate sunt dezactivate sau absente.

**Acceptare:** probele se referă la pachetul și versiunea efectiv livrate, nu doar la un build de dezvoltare. Rezultatele sunt revizuibile înaintea oricărei publicări.

## 11. M7 — Extindere controlată

- SDK de module cu manifest, compatibilitate, permisiuni, sănătate, dezactivare și actualizare.
- MCP ca adaptor de interoperabilitate, cu aceleași politici de execuție.
- Subagenți cu scope și buget limitate de sarcina părinte; utilitatea se măsoară pe sarcini de referință.
- Voce cu furnizori separabili, indicator de înregistrare și confirmarea acțiunilor relevante.
- Three.js reflectă stări reale, respectă reduced motion și are alternativă statică.
- Alte dispozitive cer identitate, pairing și transport autentificat. Produsul cu mai mulți utilizatori cere un proiect distinct de separare a datelor.
- Un eventual modul de trading are controale de domeniu independente și începe în simulare; nu face parte din fundație.

## 12. Reguli de verificare și progres

Comenzile de bază urmărite după crearea manifestelor sunt:

- Cargo: format, clippy pentru workspace/targets relevante, teste cu lockfile și build al Hostului/aplicației.
- pnpm: instalare reproductibilă cu lockfile, lint, typecheck, teste și build.
- Probe native: IPC, lifecycle, recuperare și funcții UI în executabilul Tauri.
- Probe ale furnizorilor: contract controlat, apoi integrare reală numai când este autorizată.
- Probe de distribuție: instalatorul concret, separat de surse și build.

Nu marca o etapă terminată dacă există teste obligatorii neexecutate. Pentru un blocaj, consemnează cauza, ce s-a verificat, alternativa sigură și acțiunea minimă necesară. Continuă munca independentă autorizată.

## 13. Jurnal de progres

| Data | Schimbare | Dovezi | Următorul pas |
| --- | --- | --- | --- |
| 2026-10-13 | Pachet de planificare și predare creat | Inventar local și versiuni în predare; documente, fără cod de produs | Utilizatorul poate porni M0 + M1 prin promptul pregătit |
| 2026-10-13 | Documentație verificată | 8 fișiere Markdown, 27 legături locale valide, 39 de cazuri de acceptare cu ID-uri unice; fără cod de produs | Implementarea rămâne neîncepută; cazurile de acceptare sunt specificate, nu executate |
| 2026-10-15 | Rafinare plan: candidați suplimentari pentru ADR-IPC-001 (tarpc, specta/tauri-specta) în arhitectură; notă de scop M0–M2 ferm / M3–M7 roadmap; documentele de predare/prompt generalizate (nu mai sunt legate de un agent anume, redenumite PREDARE_RO.md/PROMPT_START_RO.md) | Editări în ARHITECTURA_NEXUS_RO.md, PLAN_NEXUS_RO.md, AGENTS.md, README.md și fișierele redenumite; fără cod de produs | Implementarea rămâne neîncepută; decizie în curs privind inițializarea unui repo Git |
| 2026-10-15 | Implementare autorizată; plan M0+M1 scris și aprobat. Repo Git inițializat (primul commit: documentația). Node.js/npm/pnpm instalate real. Workspace Cargo (5 crate-uri) + `apps/desktop/src-tauri` ca membru de workspace; `cargo build --workspace` trece. Scaffold oficial Tauri v2 + React + TS în `apps/desktop`; `pnpm tauri dev` pornește fereastră nativă reală (titlu "Nexus", handle valid, verificat prin `Get-Process`, nu mock) | Commit `826ab4f`; log complet `cargo build --workspace` (Finished, fără erori); `Get-Process desktop` cu MainWindowTitle="Nexus" și MainWindowHandle valid, PID 2820 | Implementare `nexus-protocol` (ID-uri tipate, DTO-uri Handshake/GetHealth) și proba IPC named pipe + tarpc pentru ADR-IPC-001/IPC-01/IPC-02; schimbările de cod nu sunt încă în Git (necesită aprobare separată pentru commit, conform AGENTS.md) |
| 2026-10-15 | ADR-IPC-001 rezolvat: `nexus-protocol` (ID-uri, Handshake/GetHealth, ErrorCode/NexusError, trait tarpc `NexusService`, pairing token de fișier local) + `nexus-host` (bind cu lock de instanță unică, accept loop, validare token/versiune) + `nexus-client` (connect + handshake reale) implementate. 5 teste de integrare pe named pipe real (nu mock) toate PASS: IPC-01, token greșit respins, versiune incompatibilă respinsă, mesaj >1MiB respins, a doua instanță respinsă la bind | `cargo test --workspace` — 8 teste reale PASS, 0 FAIL; `cargo fmt --all -- --check` curat; `cargo clippy --workspace --all-targets` fără avertismente | Conectează shell-ul Tauri (`apps/desktop/src-tauri`) la Host prin `nexus-client`, pentru proba LIFE-01 (UI închis, Host rămâne, reconectare) și ENV-01 cu interacțiune reală; apoi `nexus-storage` (SQLite) pentru M1 |
| 2026-10-15 | Dashboardul e conectat la Host: comenzile Tauri `host_status`/`start_host`, Host pornit detașat, tipuri TS generate din Rust (`packages/contracts`), primul ecran real de stare. Host-ul are acum sesiune autentificată per conexiune, comparație de token în timp constant, log în profil și mesaj explicit pentru a doua instanță (codul de eroare `ERROR_ACCESS_DENIED` era mapat greșit) | Release `desktop.exe` + `nexus-host.exe`, rulate direct: UI „Oprit” → click real (UI Automation) pe „Pornește Hostul” → „Conectat, nexus-host, v1” → WM_CLOSE → Host-ul trăiește (PID 7180), sonda `health` răspunde (uptime 11 s) → UI-ul redeschis e conectat la același PID (uptime 15 s). Log: handshake-uri `nexus-desktop`, `nexus-probe`, `nexus-desktop`; a doua instanță are linie ERROR. 11 teste PASS, `clippy -D warnings` curat | IPC-02 „Host fals” (SID-ul serverului), apoi M1 |
| 2026-10-15 | **M0 închis.** `nexus-client` verifică SID-ul procesului care deține pipe-ul înainte să trimită tokenul (`windows-sys` direct, fără crate nou) și refuză și când identitatea nu poate fi citită. Dashboardul are starea „Neverificat” | Test pe pipe real: serverul e acceptat sub SID-ul curent și refuzat față de `S-1-5-18`. 13 teste PASS; suita IPC de 5 ori la rând fără cădere; `clippy -D warnings` curat; proba cap-coadă repetată pe release-ul reconstruit (Oprit → click → Conectat → WM_CLOSE → Host-ul trăiește → redeschis, același PID). Varianta cu alt cont Windows: BLOCKED | M1: stocare SQLite, stări, coadă, `CreateMarkdownArtifact` |
| 2026-10-15 | M1, primul punct de verificare: stările din domeniu, fundația SQLite (schema v1) și instrumentul `CreateMarkdownArtifact`, încă nelegate în Host | 33 de teste reale PASS (5 de stări; 8 de stocare pe fișiere SQLite reale; 7 pentru instrument pe NTFS real, inclusiv joncțiuni create cu `mklink /J`); `clippy -D warnings` curat. PATH-01 PASS | Runtime-ul M1 în Host |
| 2026-10-15 | M1: depozitul de date (`repo.rs`). Trimiterea idempotentă, pregătirea acțiunii cu aprobare, decizia legată de hash-ul manifestului, consumul aprobării împreună cu intenția într-o singură tranzacție, finalizarea, eșecul, revizia, expirarea, anularea, reluarea după întrerupere și snapshot-ul cu evenimente | 10 teste noi pe SQLite real, 43 în total, toate PASS; `clippy -D warnings` curat | Executorul și comenzile RPC din Host |
