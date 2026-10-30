# Securitate și validare Nexus

Acesta este un set de cerințe de proiectare și probe de acceptare. Nu este un raport de vulnerabilități descoperite și nu certifică un produs care încă nu există. Starea efectivă a etapelor se păstrează în [planul activ](PLAN_NEXUS_RO.md).

## 1. Ce protejăm și față de ce

Protejăm fișierele și conturile utilizatorului, cheile API, conversațiile, identitatea canalelor, aprobările, istoricul operațiilor și bugetele.

Intrările neîncrezătoare includ mesaje, documente, pagini web, răspunsuri de model, rezultate de instrumente, module terțe și date venite din frontend. Riscurile urmărite sunt execuția în afara scopului, scurgerea datelor, aprobările reutilizate, efectele duplicate, pierderea stării și epuizarea resurselor.

Modelul nu reprezintă o autoritate de securitate. Separarea datelor de instrucțiuni, permisiunile minime și controlul efectelor sunt cerințe de proiectare; se validează prin cazurile de mai jos. [OWASP AI Agent Security](https://cheatsheetseries.owasp.org/cheatsheets/AI_Agent_Security_Cheat_Sheet.html)

Windows, contul utilizatorului și instalarea de încredere sunt baza locală. Nu promitem protecție împotriva unui administrator ostil, a compromiterii kernelului sau a unui proces arbitrar cu toate drepturile contului utilizatorului. Workerii neîncrezători trebuie să ruleze cu restricții distincte; „este pe același PC” nu este o justificare de încredere.

## 2. Reguli verificabile

1. Toate canalele trec prin validarea identității și policy. Nu există o cale privilegiată pentru scheduler, Telegram sau extensii.
2. Domeniul unui proiect și resursele efective se verifică pe server. Frontendul nu este sursa drepturilor.
3. Aprobarea se leagă de operația exactă. Schimbarea parametrilor, actorului sau precondițiilor nu reutilizează aceeași aprobare.
4. Datele către furnizori și efectele locale/externe se verifică separat.
5. Intenția se salvează înainte de efect. Starea necunoscută este vizibilă și nu declanșează o mutație repetată automat.
6. Secretele nu apar în prompturi, outputuri, loguri sau valori recitibile din frontend.
7. Indisponibilitatea policy, seifului sau persistenței obligatorii oprește acțiunea dependentă. Nu există mod permisiv automat.
8. Anularea și oprirea nu ascund efectele deja produse.
9. Codul neîncrezător rămâne dezactivat până la demonstrarea izolării.
10. O funcție se declară funcțională numai pe baza probei relevante, pe artefactul relevant.

Tauri limitează comenzile accesibile frontendului, dar codul Rust nativ are propria responsabilitate de securitate. Testarea capabilităților UI nu înlocuiește testarea brokerului și a workerului. [Tauri security](https://v2.tauri.app/security/)

## 3. Matrice obligatorie M0/M1

Toate cazurile sunt cerințe pentru etapa indicată, nu rezultate deja obținute.

| ID | Etapă | Probă | Rezultat cerut |
| --- | --- | --- | --- |
| ENV-01 | M0 | Build și pornire a ferestrei Tauri reale, interacțiune cu Host | Aplicație nativă observată; răspuns real de la Host |
| IPC-01 | M0 | Client și server legitimi, negociere de versiune | Identitate și compatibilitate verificate |
| IPC-02 | M0 | Client neautorizat, Host fals, versiune incompatibilă, mesaj prea mare | Refuz înaintea comenzilor privilegiate; fără fallback public |
| LIFE-01 | M0/M1 | Închiderea UI în timpul unei sarcini; redeschidere | Host activ; stare recuperată și aceeași operație |
| LIFE-02 | M0 | Pornirea unei a doua instanțe Host pe același profil | Un singur proprietar; fără două cozi active |
| RUN-01 | M1 | Preview → aprobare → creare notă → verificare → deschidere | Fișierul, hashul, artefactul și starea corespund |
| RUN-02 | M1 | Refuz și expirare de aprobare | Nicio creare; stare corectă în UI și audit |
| SEC-01 | M1 | Parametri modificați, resursă schimbată, grant revocat, replay al aprobării | Operația modificată este refuzată; fără consum repetat |
| RUN-03 | M1 | Aceeași cerere repetată; aceeași cheie cu alți parametri | Aceeași operație în primul caz; conflict în al doilea |
| RUN-04 | M1 | Cădere înainte de efect și după persistarea intenției | Recuperare controlată; nu se pierde cererea |
| RUN-05 | M1 | Cădere în scriere, după publicarea fișierului și înaintea salvării rezultatului | Reconciliere fără al doilea fișier; fișierul parțial nu e succes |
| RUN-06 | M1 | Efect imposibil de atribuit sau fișier modificat înainte de recuperare | NeedsReconciliation/NeedsReview; fără suprascriere |
| RUN-07 | M1 | Anulare în coadă, în așteptare și în curs de execuție | Fără operații noi; efectele produse sunt raportate exact |
| EVT-01 | M1 | Cădere între commit și publicare; duplicate și cursor expirat | Stare/eveniment coerente; deduplicare sau refresh explicit |
| PATH-01 | M1 | Traversare, prefix asemănător, cale UNC/device, ADS și reparse/junction | Nicio scriere în afara rădăcinii; inputul arbitrar nu devine cale |
| DATA-01 | M1 | Căutarea unui secret-sentinel în loguri, erori și trafic UI; încercare de SQL/shell direct | Sentinel absent; comenzile generice indisponibile |
| DATA-02 | M1 | Disc plin, DB ocupată/coruptă, jurnal obligatoriu indisponibil | Eroare explicită; fără efect nou fără intenție durabilă; efectele începute se reconciliază |
| BACKUP-01 | M1 | Backup și restore într-un profil nou; comparație DB/artefacte | Manifest și hashuri coerente; nicio execuție reluată automat |
| UI-01 | M1 | Tastatură, focus, scalare, light/dark și stări goale/loading/offline/error | Flux utilizabil și fără controale care simulează funcții |
| UI-02 | M1 | Notă cu HTML/script, link activ, imagine remote și tentativă de apel bridge | Conținut inert; fără rețea sau comandă provocată prin randare |

Pentru PATH-01, probele OS care cer privilegii indisponibile se marchează blocate. Un test doar de normalizare textuală nu înlocuiește proba legăturii reale în filesystem.

Pentru DATA-01 în M1 se folosește un sentinel fictiv și suprafața existentă; seiful real și cheile introduse în UI se probează separat în M2. Lipsa unui seif implementat nu poate fi declarată drept test trecut al acestuia.

## 4. Probe obligatorii pentru funcțiile ulterioare

| ID | Etapă | Probă | Rezultat cerut |
| --- | --- | --- | --- |
| AI-01 | M2 | Doi furnizori cu protocoale distincte; streaming, anulare, timeout, răspuns invalid | Adaptare corectă; serverul controlat și apelul real sunt raportate separat |
| AI-02 | M2 | Instrucțiuni ostile în document/output/model; cheie-sentinel în seif | Nicio escaladare de drepturi și nicio cheie în prompt/log/UI |
| AI-03 | M2 | Endpoint/furnizor schimbat, context privat, retry și buget epuizat | Egress reevaluat; fără fallback tacit sau depășire necontrolată |
| PERSONA-01 | M2 | Română/engleză, cerere explicită de limbă, citat străin, schimbare persona/model | Regula de limbă respectată; versiune păstrată; privilegii neschimbate |
| ISO-01 | M3 | Worker încearcă acces în afara workspace-ului, rețea, seif, Host IPC și procese | Accesele neacordate refuzate de backendul real |
| ISO-02 | M3 | Proces copil, depășire CPU/memorie/timp/output, anulare și cădere | Limite aplicate întregului arbore; niciun executor abandonat cu drepturi |
| PC-01 | M3 | Țintă Windows schimbată, operație privilegiată și comandă cu argumente ostile | Precondiții și scope verificate; elevare distinctă numai când e autorizată |
| APPROVAL-01 | M2 (adus din M3 prin ADR-POLICY-001) | Rendererul încearcă să emită direct o aprobare pentru o acțiune sensibilă | Calea de confirmare de încredere este necesară; un payload UI nu ține locul deciziei umane. Un efect în afara Nexus se aprobă doar în dialogul Windows nativ afișat de aplicație; Host-ul refuză aprobarea venită din fereastră |
| SKILL-01 | M2 (ADR-PACK-001) | Skill cu scripturi, `allowed-tools`, nume greșit, fișiere care nu sunt text; skill oprit; text schimbat după pornire | Scripturile nu se păstrează și nu rulează; `allowed-tools` nu aprobă nimic; formatul greșit e refuzat cu motivul; un skill oprit nu ajunge la model; un skill pornit trimite doar numele și descrierea până e folosit; un text nou îl oprește până îl citești. **PASS** (2026-10-28): `nexus-host/tests/skills.rs`, `nexus-storage/tests/skills.rs`, `m2-ui-check.ps1` |
| PACK-01 | M2 (ADR-PACK-001) | Un anunț cu instrucțiuni ascunse ajunge în inbox; modelul cere o pagină web cu datele tale în adresă; trei schimbări într-o sarcină; pauză pentru aprobare; mesajul tău în aceeași conversație; un pachet care numește o unealtă inexistentă, un câmp necunoscut sau o limită peste maxim | Sarcina pornită de inbox nu primește citirea paginilor web, lista fișierelor, uneltele MCP, faptele de memorie, notele și fișierele noi; o cerere a modelului pentru ele e refuzată cu motivul și nimic nu pleacă; două schimbări se scriu, a treia așteaptă aprobarea; regulile țin și după pauză; mesajul tău rulează doar după regulile tale; un pachet greșit e refuzat. **PASS** (2026-10-30): `nexus-host/tests/inbox.rs` (2 teste noi), `runtime/packs.rs`, `runtime/permissions.rs`, `m2-ui-check.ps1` |
| AGENT-01 | M2 (ADR-AGENT-001) | Un agent cu web-ul oprit căruia modelul îi cere totuși o pagină; un agent cu numele „Nexus”, cu o unealtă sau un skill inexistent, cu o conexiune fără model; o editare pe o revizie veche; un agent arhivat; o continuare în aceeași conversație; o sarcină fără agent | Unealta oprită nu e oferită, iar cererea e refuzată cu motivul și nimic nu pleacă; ce nu există e refuzat; o revizie veche e refuzată; un agent arhivat nu pornește nimic nou, iar sarcinile lui îi păstrează numele; continuarea rămâne cu agentul ei; agentul vede doar skill-urile alese, modelul și proiectul lui; fără agent, „Nexus” e neschimbat; ce scrie agentul poartă actorul lui. **PASS** (2026-10-30): `nexus-host/tests/agents.rs`, `nexus-storage/tests/agents.rs`, `runtime/agents.rs`, `runtime/permissions.rs`, `m2-ui-check.ps1` |
| CODE-01 | M3 | Proiect cu modificări existente; propunere de patch și teste | Modificările utilizatorului păstrate; diff inspectabil; fără publicare implicită |
| BROWSER-01 | M3 | Pagină cu prompt injection, download, redirect și formular cu efect extern | Profil și scope controlate; acțiunea externă cere drepturile necesare |
| AUTO-01 | M4 | Schimbare de oră, PC oprit, overlap, retry și grant revocat | Comportament de calendar definit; fără rafale sau efecte duplicate |
| CHANNEL-01 | M4 | Telegram: update duplicat, alt ID, mesaj redirecționat, callback expirat | Identitatea și aprobarea sunt verificate; niciun drept din text |
| MEM-01 | M5 | Căutare între proiecte, ștergere/corectare, embedding schimbat, import supradimensionat | Scope respectat; indexuri coerente; sursă vizibilă și cote aplicate |
| DATA-03 | M6 | Backup sensibil, cheie lipsă, export portabil și restore | Protecția declarată demonstrată; pierderea cheii are rezultat explicit |
| RELEASE-01 | M6 | Instalare curată, upgrade, eșec la migrare, incompatibilitate și dezinstalare | Date păstrate sau șterse conform alegerii; revenire verificată |
| RELEASE-02 | M6 | Pachet alterat, actualizare incompatibilă, worker activ | Verificări de integritate/compatibilitate; nicio înlocuire nesigură |
| EXT-01 | M7 | Modul incompatibil, permisiuni suplimentare, dezactivare și subagent | Refuz sau aprobare distinctă; scope și buget limitate |
| VISUAL-01 | M7 | Three.js ascuns, reduced motion și efecte dezactivate | Alternativă utilizabilă; consum oprit/redus și stări reale |
| VOICE-01 | M7 | Microfon oprit, transcriere ambiguă, replay audio | Nicio captare ascunsă; acțiunea sensibilă nu se aprobă doar prin text dedus |

Nu activăm o funcție dependentă de un caz blocat doar pentru a afișa un dashboard complet.

## 5. Metoda de probă

Testele automate folosesc proiecte și profiluri temporare dedicate, servere controlate și date fictive marcate. Fault injection se face la granițe persistente: înainte/după tranzacție, înainte/după efect, înainte/după publicarea evenimentului.

Pentru recuperare trebuie cel puțin o probă cu procese reale oprite la punctele definite; un mock al bazei de date nu demonstrează durabilitatea SQLite sau relația cu filesystemul.

O bază coruptă se păstrează pentru diagnostic/restaurare; nu se șterge și nu se reinițializează automat pentru a ascunde eroarea. Un test de corupție operează numai asupra copiei dedicate probei.

Nu folosim fișiere personale, conturi reale, apeluri plătite sau modificări de sistem pentru teste fără autorizația aplicabilă. Testele nu dezactivează Windows Application Control.

La execuție, fiecare caz primește PASS, FAIL, BLOCKED sau NOT RUN, cu:

- buildul/revizia și mediul;
- comanda sau pașii executați;
- rezultatul observat și artefactul de probă redactat;
- limita probei și remedierea necesară.

Rezultatele detaliate pot fi artefacte generate, cu retenție limitată. Planul conține rezumatul și legăturile lor, fiind unicul registru de progres. Fixtures, chei-sentinel și rapoarte locale nu intră în pachetul livrat.

## 6. Comenzi țintă după bootstrap

Aceste comenzi sunt o țintă pentru scripturile viitoare, nu dovezi că există sau au rulat. Numele pachetului desktop se fixează în M0; propunerea este @nexus/desktop.

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
pnpm.cmd install --frozen-lockfile
pnpm.cmd -r lint
pnpm.cmd -r typecheck
pnpm.cmd -r test
pnpm.cmd -r build
pnpm.cmd --filter @nexus/desktop tauri build --no-bundle
```

La prima generare a lockfile-ului se folosește procedura normală de instalare după aprobarea dependențelor. Verificarea frozen se face după ce există un lockfile coerent.

Scripturile fiecărui pachet relevant trebuie să existe și să ruleze; nu folosim ignorarea scripturilor absente pentru a raporta succes. La M0 se stabilește ordinea exactă pentru frontend, Host și shell, inclusiv copierea artefactului Host compatibil în buildul de dezvoltare.

Un build web verifică frontendul. Buildul Tauri verifică integrarea de compilare. Pornirea și interacțiunea native verifică execuția. Installerul și versiunea instalată sunt artefacte distincte și au probe separate.

Dacă accesul la fereastra nativă sau un instrument de testare lipsește, se raportează exact ce nu s-a probat. Un screenshot din browser nu este prezentat drept dovadă Tauri.

## 7. Performanță și spațiu pe disc

Nu stabilim cifre de marketing înainte de măsurare. Înregistrăm separat:

- instalatorul descărcat și aplicația instalată;
- Rust/Node, cache-uri, dependențe și builduri de dezvoltare;
- baza de date, artefactele, logurile, backupurile și indexurile;
- runtime-urile și greutățile modelelor locale opționale.

Măsurăm întregul arbore de procese: Host, Tauri/WebView2 și workers. Scenariile sunt idle, conversație, operație PC și vizualizare activă/inactivă. Notăm CPU, RAM, GPU, timp până la fereastră utilizabilă și timpul de finalizare.

Cotele și retenția sunt configurabile în limite valide; disk-full și backpressure au teste. Dimensiunea unui executabil nu reprezintă amprenta totală a produsului.

Estimare inițială orientativă, neconfirmată prin măsurare: aplicația instalată (Tauri peste WebView2 partajat de sistem, deja prezent pe majoritatea instalărilor Windows 11) se așteaptă în zona 20–100 MB; baza SQLite și artefactele cresc lent cu utilizarea reală, de ordinul zecilor de MB pe termen scurt. Mediul de dezvoltare (toolchain Rust/Node, cache-uri, builduri de dezvoltare) poate ajunge la câțiva GB, dar nu face parte din produsul livrat utilizatorului final. Greutățile unui model local opțional rămân cel mai mare consumator posibil — de la câteva sute de MB la zeci de GB — motiv suplimentar pentru care arhitectura preferă API-uri de furnizor (secțiunea M5 din plan) înaintea unor modele locale grele.

## 8. Pragul de livrare

O etapă este completă când criteriile ei sunt demonstrate, blocajele sunt rezolvate sau funcțiile opționale excluse explicit, iar utilizatorul poate reproduce fluxul promis. Raportul distinge cod scris, build, probe automate, UI nativă și pachet instalat.

Un produs „sigur” nu înseamnă risc zero. Înseamnă limite explicite, comportament verificat în condițiile declarate, actualizări controlate și raportarea onestă a incertitudinii.
