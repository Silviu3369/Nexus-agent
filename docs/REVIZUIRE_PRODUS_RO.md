# Revizuire de produs și arhitectură — propunere

Data: 2026-10-19. **Status: ajustările adoptate de tine la 2026-10-20 și trecute în [plan](PLAN_NEXUS_RO.md); nișa comercială (DocumentOps sau IT Support) se decide după Jobs R1.**

- [Planul activ](PLAN_NEXUS_RO.md) rămâne singurul registru de progres și singura sursă a roadmap-ului.
- Nimic de aici nu se aplică până nu decizi. Nu s-a scris cod pentru această revizuire.

**Teza evaluată:** Nexus este o platformă locală de execuție supravegheată a muncii, pe Windows: face muncă reală pe calculator, iar omul păstrează controlul asupra acțiunilor cu consecințe.

**Trei straturi:**

- **Nexus Core** — platforma reutilizabilă de execuție și securitate.
- **Nexus Jobs** — primul mediu de folosire reală (dogfooding); nu neapărat nișa comercială finală.
- **Nexus DocumentOps** — prima pană comercială evaluată: un agent Windows controlat pentru munca repetitivă de back-office, plină de documente.

## Verdict pe scurt

- **Roadmap-ul M0–M7 rămâne compatibil cu teza.** Nu e nevoie de un roadmap nou, de milestone-uri noi sau de renumerotare. Teza e, în mare, schema logică deja scrisă în [arhitectură](ARHITECTURA_NEXUS_RO.md) §3: Host → Broker → workeri și conectori → verificare → audit. S-a schimbat prioritatea, nu destinația.
- **Trei ajustări reale:**
  1. **M2 nu e complet pe un punct pe care îl conține deja:** „integrează propunerile de instrumente ale modelului cu același broker din M1”. Munca agentului nu trece printr-un singur model Task/Run/Step/Action și nici printr-un singur broker. Fără asta, nicio metrică cerută la H nu e credibilă.
  2. **M3 își schimbă conținutul.** Codul a ieșit deja (decizia ta, 2026-10-17). Intră operațiile tipate pe fișiere și Office și revine izolarea pentru parsere. Browserul rămâne ultimul.
  3. **Emailul lipsește din toate milestone-urile**, deși e intrarea principală pentru DocumentOps și pasul 5 din §3a. Trebuie inserat în M4.
- **Ordinea de execuție recomandată nu urmează numerotarea.** Asta se întâmplă deja: Memorie v1 (M5), MCP și Nexus Core 3D (M7) au fost aduse înainte. Numerotarea rămâne.
- **Șapte conflicte între direcția nouă și ce există** (secțiunea „Conflicte”), cu recomandarea pentru partea care trebuie să cedeze.
- **DocumentOps se potrivește arhitecturii mai bine decât IT Support**, nișa aleasă azi. Recomand DocumentOps ca pană comercială și IT Ops ca pachet ulterior. Decizia e a ta.

## A. Roadmap-ul actual, exact cum e în proiect

Sursa: [plan](PLAN_NEXUS_RO.md) §3 (tabelul etapelor), §4–§11 (conținutul etapelor) și §3a (direcția agentului).

| Etapă | Rezultat (din plan) | Conținut esențial (din plan) | Acceptare (din plan) | Stare (din plan) |
| --- | --- | --- | --- | --- |
| M0 | Mediu verificat, workspace și transport local demonstrat | workspace Cargo/pnpm; Host separat, o instanță per profil; conexiune autentificată; negocierea versiunii; proba transportului; fereastra Tauri reală | IPC tipat; versiuni și clienți neautorizați respinși; Host-ul supraviețuiește închiderii UI | Închis (IPC-02 cu alt cont Windows: BLOCKED) |
| M1 | Circuit local complet, persistent și recuperabil | tipuri Project/Task/Run/Step/Action/Approval/Artifact/Event; SQLite cu evenimente în aceeași tranzacție; coadă persistentă; politică restrictivă și `CreateMarkdownArtifact`; aprobare legată de manifestul exact; intenție înaintea efectului, verificare, reconciliere; backup și restaurare; audit și teste cu defecte injectate | matricea M0/M1; nicio dublare după cădere; efectul nedemonstrat rămâne incert | Complet tehnic; UI-01 vizual în așteptare |
| M2 | Conversație AI reală, modele selectabile, personalitate | Gemini, apoi OpenAI, DeepSeek, endpoint local; catalog de capabilități; seif; context autorizat; personalitate versionată; bugete de tokeni, timp și pași; **propunerile modelului prin același broker din M1**; rezultatul modelului nu poate emite o aprobare | adaptorul corect; anulare, limită, timeout testate; chei absente din UI și loguri; fără schimbare tacită de furnizor | Închis funcțional: Gemini verificat real; OpenAI, Claude, DeepSeek doar pe server controlat; reîncercare, rezervă explicită, buget zilnic, cost estimat |
| M3 | ~~Agent de programare~~, PC și browser cu izolare verificată | adaptor Windows tipat; cod neîncrezător doar în izolare demonstrată; ~~programare~~; browser cu profil dedicat; Python/PowerShell doar ca instrumente; mouse/tastatură cu verificarea țintei; cale de confirmare de încredere; probe de acces refuzat | un flux real de cod (scos) și o operație PC verificată prin policy, broker, audit și verificare | Agentul de programare scos (2026-10-17); izolarea (ADR-ISO-001) probată, apoi codul scos (rămâne în commit `88ebc89`); PC și browser neîncepute |
| M4 | Automatizări durabile și Telegram | scheduler cu fus orar, ore schimbate, programări ratate, suprapuneri, revocare; Telegram cu long polling, asociere explicită, deduplicare; WhatsApp condiționat | duplicatele nu dublează efectele; un program ratat nu face rafală; revocarea oprește | Automatizări făcute (2026-10-18); Telegram neînceput |
| M5 | Memorie, documente și căutare cu proveniență | preferințe, cunoștințe pe proiect, context temporar; proveniență, păstrare, ștergere; căutare textuală, embeddings doar dovedite; importul documentelor cu limite și izolare pentru parsere | memoria nu trece între proiecte; ștergerea ajunge în indexuri; răspunsurile pot indica sursele | Parțial: Memorie v1 făcută. Rândul din §3 spune încă „documente neîncepute”, dar ele s-au făcut prin §3a: foldere, citire PDF/Word/Excel, scrisoare Word, Excel, evidențe |
| M6 | Distribuție Windows calificată pentru utilizare zilnică | profiluri, criptare la repaus (ADR-DATA-001), pachet, semnare, actualizare, dezinstalare, audit de dependențe, consum, diagnostic | probe pe pachetul livrat | Neînceput |
| M7 | Extensii avansate, voce, Three.js, alte dispozitive | SDK de module; MCP ca adaptor; subagenți; voce; Three.js; alte dispozitive; produs multi-utilizator ca proiect separat; trading doar în simulare | — | Parțial: Nexus Core în Three.js; MCP ca client |

**Direcția agentului (§3a, 2026-10-18).**

- **Trei componente comune:** Evidențe; Acces la date; Documente și acțiuni.
- **Ordinea pașilor:**
  1. Evidențe + Jobs — făcut.
  2. Foldere și documente — făcut; apoi inbox, scor față de CV și Results — făcute.
  3. Google Drive.
  4. Telegram.
  5. Email.
  6. Restul modulelor; operațiile PC (M3) și browserul (ultimul) rămân în plan.
- **Nișa aleasă azi:** căutare de job asistată + IT support pentru firme mici.

## B. Fiecare milestone față de strategie

Fiecare intrare răspunde la întrebările cerute:

- dacă rămâne neschimbat sau se ajustează;
- dacă ceva se mută mai devreme sau mai târziu;
- dacă un punct a devenit inutil;
- dacă se inserează o cerință nouă;
- dacă ordinea mai are sens arhitectural.

### M0 — Mediu, workspace și transport

- **Scop inițial:** fundația tehnică demonstrată.
- **Stare:** închis. IPC-02 cu alt cont Windows rămâne BLOCKED: cere un al doilea cont Windows pe PC.
- **Relevanță:** totală, ca fundație. Proba cu alt cont contează doar dacă Host-ul ar servi alți utilizatori Windows, ceea ce nici DocumentOps nu cere, cu o stație per operator.
- **Decizie: păstrat neschimbat.** Nicio cerință nouă. Ordinea e corectă.

### M1 — Primul circuit complet

- **Scop inițial:** circuitul supravegheat: propunere → aprobare exactă → intenție → efect → verificare → audit → recuperare.
- **Stare:** complet tehnic; verificarea vizuală UI-01 rămâne deschisă (minoră).
- **Relevanță:** maximă. E chiar inima tezei: „omul păstrează controlul asupra acțiunilor cu consecințe”.
- **Decizie: păstrat neschimbat, fără redeschidere.** Modelul M1 e folosit doar pentru efectele cu aprobare (nota, documentul, apelul MCP). Asta nu e un defect al lui M1; e partea neterminată din M2 (mai jos).

### M2 — Furnizori AI și personalitate

- **Scop inițial:** AI real, modele alese explicit, personalitate. Conține explicit două lucruri:
  - „integrează propunerile de instrumente ale modelului cu același broker din M1”;
  - „bugete de tokeni, timp și număr de pași; cost estimat”.
- **Stare:** declarat „închis funcțional”. În cod:
  - munca agentului e conversație → mesaj → `agent_steps`; tabelul `tasks` din M1 servește doar notelor;
  - scrierile agentului în evidențe nu trec prin Action/broker; sunt directe, cu Undo, prin decizia ta, care e bună ca politică;
  - costul e înregistrat pe zi și pe model (`model_usage`), nu pe task;
  - o sarcină nu are un rezultat final: un fișier din inbox „Done” înseamnă doar că task-ul s-a terminat, nu că a reușit.
- **Relevanță:** maximă. Toate metricile de la H depind de un model unic al sarcinii și de un singur broker.
- **Decizie: ajustat.** De ce e o schimbare substanțială: fără ea, Nexus raportează conversații, nu sarcini. „Rata de reușită pe task” nu poate fi calculată cinstit. De ce nu e un milestone nou: cerința e deja scrisă în M2; se termină, nu se inventează.
- **Ce se inserează în M2:**
  - modelul Task unic (obiectiv, declanșator, rezultat, motivul eșecului);
  - conversația devine un canal prin care se creează sarcini;
  - toate efectele trec prin brokerul M1, fiecare cu o clasă de efect (secțiunea I);
  - costul și timpul se atribuie fiecărei sarcini;
  - taxonomia eșecurilor.
- **Tot în M2: politica de date pe furnizor.** Planul notează deja că nivelul gratuit Gemini poate folosi datele pentru îmbunătățirea modelului. Pentru documentele unei firme, asta e inacceptabil. Înaintea oricărui pilot, un furnizor plătit, cu acord de prelucrare a datelor (DPA), trebuie probat real. Punctul e listat deja în M2 ca NOT RUN.
- **Ordinea:** completarea M2 vine prima, înaintea oricărui pachet nou.

### M3 — Cod, PC și browser

- **Scop inițial:** acțiuni pe PC și în browser, cu cod neîncrezător doar în izolare demonstrată.
- **Stare:**
  - programarea e scoasă din scop (decizia ta);
  - izolarea AppContainer a fost probată, apoi codul a fost scos (commit `88ebc89`);
  - operațiile PC și browserul sunt neîncepute.
- **Relevanță:** mare, dar alt conținut. „Muncă reală pe calculator” pentru DocumentOps înseamnă fișiere, documente Office, email și, mai rar, aplicații web vechi. Nu administrarea generală a PC-ului și nici cod.
- **Decizie: modificat substanțial.** De ce:
  1. codul a ieșit deja prin decizia ta, deci jumătate din M3 nu mai există;
  2. DocumentOps are nevoie mai întâi de operații tipate pe fișiere și Excel, cu verificare și rollback;
  3. browserul e componenta cea mai riscantă: termenii platformelor, CAPTCHA pe care nu o ocolim, selectori fragili, instrucțiuni ascunse în pagini. Decizia veche „browserul ultimul” rămâne corectă.
- **Conținut propus, în același M3, fără renumerotare:**
  - **M3a — operații tipate pe fișiere și Office** în folderele împărțite: mutare, redenumire, copiere; actualizarea unui Excel existent cu diferența afișată înainte, verificare după și copie pentru rollback.
  - **M3b — izolarea readusă** (implementarea ADR-ISO-001 din `88ebc89`) pentru parsere, OCR și orice unealtă neîncrezătoare. Cititorul de documente de azi rulează într-un Job Object: limitează resursele, dar nu e o izolare (AGENTS.md o spune explicit). Pentru documentele unor terți, procesate în volum, izolarea devine necesară.
  - **M3c — browser cu profil dedicat:** întâi citire, apoi completarea formularelor cu aprobarea valorilor exacte trimise; doar domenii permise. Niciodată platforme care interzic automatizarea (de exemplu LinkedIn) și niciodată CAPTCHA.
- **Amânat după prima versiune:** automatizarea generică cu mouse/tastatură a aplicațiilor desktop. E teritoriul RPA, foarte fragil și greu de verificat.
- **Inutil:** fluxul de acceptare „modificare și testare de cod”.

### M4 — Automatizări și Telegram

- **Scop inițial:** muncă pornită de timp și de canale, nu doar de un mesaj în fereastră.
- **Stare:** automatizările sunt gata; Telegram nu e început. Inboxul de anunțuri (Jobs) e de fapt un declanșator bazat pe evenimente, făcut în §3a.
- **Relevanță:** mare. Declanșatoarele fac diferența dintre agent și chatbot.
- **Decizie: ajustat.**
- **Se inserează:**
  1. **Intake generic:** surse folder și email; fișier luat doar când e stabil; deduplicare după conținut; un task per element; reîncercare cu așteptare. Generalizează inboxul Jobs, care e azi legat de un registru.
  2. **Conectorul de email:** citire ca sursă de intake, draft, trimitere doar cu aprobare, verificare în Sent după ID-ul mesajului. Lipsea din toate milestone-urile.
  3. **Remindere din evidențe:** o dată de follow-up ajunsă la scadență devine o notificare sau un task. Azi automatizările sunt doar instrucțiuni la oră fixă.
- **Mutat mai târziu în M4:** Telegram, după email. E util pentru folosirea ta personală, are valoare mică pentru B2B, iar aprobarea de pe telefon cere calea de confirmare de încredere (arhitectura §11).
- **WhatsApp:** neschimbat.

### M5 — Memorie și documente

- **Scop inițial:** memorie cu proveniență; documente importate în siguranță; răspunsuri cu surse.
- **Stare:**
  - Memorie v1 e făcută;
  - prin §3a s-au făcut: foldere împărțite cu reguli de cale, citire PDF/Word/Excel/CSV în proces separat cu limite, scrisoare Word cu aprobare, import/export Excel, evidențe cu istoric și Undo, inbox, scor față de CV, Results;
  - lipsesc: căutarea cu sursa arătată, retenția, sursele în răspunsuri.
- **Relevanță:** cea mai mare. Nucleul DocumentOps e aici.
- **Decizie: ajustat și adus mai devreme.**
- **Se inserează:**
  1. **Extracție structurată** după o schemă declarată, cu proveniență pe câmp (fișier, pagină, fragment) și cu semnalarea câmpurilor nesigure.
  2. **Reguli de validare deterministe în Host:** formate, totaluri, verificare față de evidențe sau un Excel de referință. Separate de model.
  3. **Clasificarea** documentelor primite.
  4. **OCR** pentru PDF-uri scanate și imagini, prin OCR-ul Windows. Fără dependență mare nouă; disponibilitatea pe Windows Home trebuie verificată.
  5. **Retenție** pe clasă de document.
- **Evidențele intră formal în M5**, ca stare structurată cu proveniență și Undo.
- **Neschimbat:** embeddings rămân amânate; regulile memoriei rămân.

### M6 — Calificare pentru utilizare zilnică

- **Scop inițial:** pachet instalabil, date protejate, actualizare sigură.
- **Stare:** neînceput.
- **Relevanță:** obligatoriu înaintea oricărui pilot comercial; nu e necesar pentru dogfood.
- **Decizie: păstrat, cu priorități clare:**
  - ADR-DATA-001 (criptare la repaus, recuperarea cheilor, retenție, export) se *decide* acum (secțiunea I) și se *implementează* aici;
  - exportul auditului și diagnosticul fără date sensibile devin criterii explicite;
  - ordinea rămâne: după prima versiune Jobs de dogfood.

### M7 — Extindere controlată

- **Scop inițial:** extensii (module, MCP, subagenți, voce, Three.js, alte dispozitive).
- **Stare:** parțial — Three.js și MCP-client aduse înainte.
- **Relevanță:** un SDK pentru terți e irelevant pentru pana comercială și contrar lecției OpenClaw (skill-uri terțe malițioase). Pachetele first-party (Jobs, DocumentOps, mai târziu FinanceOps, RecruitingOps, IT Ops) sunt însă exact „modulele” din M7.
- **Decizie: ajustat ușor.**
  - **Adus înainte:** doar *formatul intern de pachete first-party*, declarativ și fără cod, când apare al doilea pachet. E același precedent ca la MCP.
  - **Rămân amânate:** SDK-ul pentru terți, subagenții, vocea, alte dispozitive, trading.
  - **Three.js:** înghețat, fără investiții noi.
  - **Produsul multi-utilizator:** rămâne proiect separat (neschimbat).

### Mai are sens ordinea, arhitectural?

- **Dependențele tari rămân valabile:**
  - M0 → M1 → M2;
  - M3c (browser) și orice unealtă neîncrezătoare depind de M3b (izolare);
  - aprobarea de pe telefon (M4) depinde de calea de confirmare de încredere;
  - parsarea în volum (M5) depinde de M3b.
- **Între M3, M4 și M5 nu există dependență tare în ordinea numerelor.** De aceea propun o ordine de execuție, nu o renumerotare:
  1. completarea M2;
  2. M5 (extracție și validare) în paralel cu M4 (intake, email);
  3. Jobs R1 (evaluare);
  4. M3a/M3b;
  5. M6;
  6. pilot DocumentOps;
  7. M3c;
  8. M7.

## C. Componente existente care susțin direct strategia

| Componentă | Unde | Susține |
| --- | --- | --- |
| Host independent, o instanță per profil, pipe cu sesiune autentificată | M0 | Munca nu se oprește la închiderea ferestrei; nicio suprafață de rețea |
| Stare durabilă, intenție înaintea efectului, reconciliere, chei de idempotență, injectare de defecte | M1 | „Crash recovery and idempotency” — probat cu procese reale oprite |
| Aprobare legată de manifestul exact, o singură folosire, expirare | M1; documente; apeluri MCP | „Action-level approvals” |
| Verificare după execuție (SHA-256 după scriere; NeedsReview la nepotrivire) | M1; documente | „Verification after execution” (pentru fișiere) |
| Istoric pe evidențe, cu Undo într-un clic | §3a / M5 | „Undo / rollback” (pentru starea internă) |
| Evenimente persistente, Activity, pașii agentului | M1; M2 | „Persistent event / audit history” (parțial: fără hash în lanț) |
| Seif în Credential Manager, curățat la pornire | M2 | „Secret vault” (cu limita cunoscută la scrieri concurente, plan problema 6) |
| Adaptoare Gemini, OpenAI, Anthropic, DeepSeek; rezervă explicită; buget zilnic; cost estimat | M2 | „Provider-independent routing” (doar Gemini verificat real) |
| Memorie v1 cu proveniență și aprobare | M5 | „Memory” |
| Evidențe cu schemă, validare, câmp-cheie (fără dubluri), istoric | §3a / M5 | Starea structurată de care DocumentOps are nevoie |
| Citirea documentelor într-un proces separat, cu limite de memorie, procese și timp | §3a / M5 | Punctul de plecare pentru workeri (încă fără izolarea accesului) |
| Foldere împărțite, cu căile sistemului, legăturile și rețeaua refuzate | §3a / M5 | „Workers with scoped permissions” (la nivel de cale) |
| Document creat doar după aprobare, cu staging și verificare | §3a / M5 | Prima acțiune externă reală, verificată |
| Import Excel cu maparea arătată înainte; export | §3a / M5 | Pasul „update Excel” (azi doar fișiere noi, nu actualizare pe loc) |
| Inbox: fișier stabil, deduplicare după hash, un task per fișier, reîncercare, buget | §3a / M4 | Prototipul intake-ului generic |
| Scheduler cu ora locală, recuperare limitată, idempotență | M4 | Muncă pornită de timp |
| MCP: citiri automate, efecte aprobate apel cu apel | M7 (adus înainte) | Conectori fără cod terț local |
| Results (Jobs) | §3a | Metrici de rezultat (nu încă de execuție) |
| Teste: furnizori falși, verificări UI Automation pe fereastra reală, defecte injectate | transversal | Dovezi de calitate pe care un client B2B le poate cere |

## D. Ce lipsește

1. **Modelul unic al sarcinii, cu rezultat.** Azi sunt două modele paralele: `tasks/runs/actions` (M1) și `conversations/messages/agent_steps`.
2. **Brokerul unic și taxonomia efectelor.** Fiecare unealtă are drumul ei. Scrierile în evidențe nu sunt Actions.
3. **Telemetria de execuție:** cost și timp pe task, intervenții umane, timpul de decizie la aprobare, motivele eșecului, rezultatul.
4. **Conectorul de email.**
5. **Extracția structurată:** schemă, proveniență pe câmp, reguli de validare, clasificare, OCR.
6. **Operațiile tipate pe fișiere și actualizarea pe loc a unui Excel**, cu verificare și rollback.
7. **Izolarea pentru parsere și unelte neîncrezătoare** (cod scos, recuperabil).
8. **Intake-ul generic** (azi doar inboxul Jobs).
9. **Formatul de pachet.** Jobs e scris direct în Host: schema în `records.rs`, statusurile și calculul în `job_results.rs`, adăugat azi. E datorie tehnică asumată.
10. **Politica de aprobare ca date:** reguli și praguri pe flux; aprobări în lot cu diferențe; calea de confirmare de încredere cerută de arhitectură înainte de M3.
11. **Clasificarea datelor pentru furnizori:** ce câmpuri pot pleca; mascarea IBAN-ului și a actelor de identitate; fără nivel gratuit pentru date de business.
12. **Criptare la repaus, retenție, export de audit** (ADR-DATA-001, M6).
13. **Identitatea actorului** dincolo de „User/Agent”, ca ulterior să se poată separa operatorul de cel care aprobă.
14. **Browserul.** Nu e necesar pentru prima versiune.

## E. Ce se amână (nu contribuie la prima versiune cu adevărat utilizabilă)

| Element | Milestone | Motiv |
| --- | --- | --- |
| Telegram | M4 (după email) | Confort personal; aprobările de pe telefon cer întâi calea de încredere |
| WhatsApp | M4 | Deja condiționat |
| Voce, alte dispozitive, subagenți, trading, SDK pentru terți | M7 | Fără legătură cu pana comercială; riscul OpenClaw |
| Investiții noi în Three.js și în aspectul „command center” | M7 / UI | Riscul „frumos, nu util”; partea făcută rămâne |
| Automatizarea generică cu mouse/tastatură | M3 | RPA fragil, greu de verificat |
| Trimiterea formularelor din browser | M3c | După email; în Jobs R1 aplici prin email sau manual |
| Embeddings / căutare vectorială | M5 | Deja amânate; căutarea textuală ajunge |
| Proba reală OpenAI/Claude/DeepSeek | M2 | Doar când un pilot cere un furnizor plătit cu DPA; atunci devine obligatorie |
| IPC-02 cu alt cont Windows | M0 | Nu e necesar (stație per operator) |
| „Pasul 3: Google Drive” | §3a | E doar un folder împărțit; se verifică la ocazie, fără muncă dedicată |
| Modulul IT Support | M7 (pachet) | Dacă alegi DocumentOps ca pană |

## F. Capacitatea minimă pentru a fi „agent”, nu „dashboard AI”

**Definiție testabilă.** Nexus pornește de la un eveniment observat singur, nu de la un mesaj în chat. Termină o sarcină în mai mulți pași care se încheie cu cel puțin un efect cu consecințe în afara propriei baze de date. Pe drum:

1. intrarea e conținut real, neîncrezător (document sau email);
2. agentul își alege uneltele; rezultatele intermediare sunt structurate și validate de reguli deterministe, nu doar de model;
3. fiecare efect cu consecințe trece prin un singur broker: previzualizare exactă, aprobare după politică, intenție salvată, execuție o singură dată, verificare față de realitate după execuție;
4. eșecurile sunt clasificate și reîncercate doar când e sigur; efectele incerte sunt reconciliate, niciodată repetate orbește;
5. omul vede, corectează, respinge sau anulează, cât permite clasa efectului;
6. sarcina se încheie cu rezultatul, costul, timpul și intervențiile înregistrate.

**Azi:**

| Criteriu | Stare |
| --- | --- |
| 1. Intrare reală, neîncrezătoare | Îndeplinit (inbox, PDF) |
| 2. Rezultate validate determinist | Parțial: validare pe câmpuri, nu reguli de flux |
| 3. Un singur broker pentru efecte | Îndeplinit pentru fișiere și MCP; nu pentru evidențe, email, browser |
| 4. Eșecuri clasificate, fără repetare oarbă | Îndeplinit |
| 5. Omul poate corecta sau anula | Îndeplinit pentru evidențe |
| 6. Rezultat, cost, timp înregistrate | Neîndeplinit |

**Verdict cinstit:** azi Nexus e un agent cu efecte în principal *interne*, nu încă o platformă de execuție a muncii. Piesele decisive care lipsesc: **un efect extern cu consecințe** (trimiterea unui email sau actualizarea pe loc a unui fișier sau Excel) și **rezultatul sarcinii**.

**Proba de acceptare propusă (Jobs):**

1. Sosește un email cu un anunț.
2. Se creează un task.
3. Extracție și scor față de CV.
4. Scrisoare Word adaptată.
5. Aprobare.
6. Emailul de aplicare pleacă la adresa din anunț, dacă anunțul cere aplicare prin email.
7. Verificare în Sent.
8. Candidatura trece în „Applied”.
9. Follow-up programat.
10. Răspunsul companiei, prin email, propune noul status.
11. Rezultatul sarcinii e înregistrat.

**Echivalentul DocumentOps:**

1. Factură primită prin email.
2. Extracție.
3. Validare față de lista de furnizori.
4. Diferența propusă pentru registrul Excel.
5. Aprobare.
6. Scriere.
7. Verificare.
8. Audit.

## G. Jobs R1 (dogfood) pe milestone-urile existente

| Etapa fluxului | Azi | Milestone | Ce trebuie pentru R1 |
| --- | --- | --- | --- |
| Descoperirea joburilor | Folderul-inbox ✓ | M4 (intake) | Email ca sursă; surse oficiale doar unde sunt permise (decizie §3a); fără căutare automată în browser |
| Analiza anunțului | ✓ | M5 | Proveniență pe câmp |
| Potrivirea (fit) | Scor față de CV ✓ | M5 | Explicație cu citate din anunț și din CV |
| Generarea documentelor | Scrisoare Word cu aprobare ✓ | M5 | Opțional: varianta de CV adaptată |
| Aprobarea | ✓ | M1 / M2 (broker) | Previzualizare mai clară; aprobări în lot |
| Acțiuni controlate în browser | — | M3c | **Nu în R1** (motivul la Conflicte, punctul 2). În R1 aplici prin email (M4) sau manual, iar Nexus înregistrează. Browserul vine în R2 |
| Evidența candidaturilor | ✓ | M5 | — |
| Follow-up | Automatizări ✓ | M4 | Remindere generate din datele de follow-up; răspunsurile prin email actualizează statusul |
| Metrici | Results ✓ (rezultat) | M2 (completare) | Telemetria de execuție (H) — obligatorie pentru R1 |

**Criterii de ieșire pentru R1:**

- **Volum:** cel puțin 30 de anunțuri reale, pe cel puțin 3 săptămâni (praguri de confirmat de tine).
- **Ce se măsoară:**
  - corectitudinea extracției (corecturile tale pe candidatură);
  - rata de reușită pe task;
  - intervențiile pe task;
  - costul pe anunț;
  - timpul median de la anunț la candidatura pregătită.
- **Obligatoriu:** zero efecte cu consecințe neaprobate și zero trimiteri duble.

**Limită:** o singură persoană (n=1), iar căutarea ta de job se poate încheia. Fixează o fereastră de evaluare. Ca a doua sursă de dovezi, mai apropiată de DocumentOps, poți folosi Nexus pe propriile documente administrative: facturi, bonuri, abonamente.

## H. Telemetrie și metrici

**Principii:**

- se calculează local, din starea durabilă; nimic nu pleacă de pe PC fără export cerut de tine;
- fiecare metrică are o definiție exactă;
- o estimare e etichetată ca estimare.

**Fapte durabile noi, în completarea M2:**

- **Task:** flux sau pachet, sursa declanșării, început, sfârșit, rezultat (Succeeded / Failed / Abandoned / Cancelled), motivul eșecului.
- **Action:** clasa efectului, rezultatul verificării, compensarea.
- **Evenimente de intervenție:** aprobare, respingere, editare înainte de aprobare, Undo, corectură ulterioară, răspuns la o întrebare a agentului, reîncercare manuală, preluare manuală.
- **Segmente de timp:** agentul lucrează / așteaptă omul / așteaptă furnizorul.

| Metrică | Definiție | Sursă azi | Lipsă |
| --- | --- | --- | --- |
| Rata de reușită pe task | Task-uri Succeeded / task-uri încheiate. Succeeded = toate acțiunile verificate *și* nicio respingere, Undo sau corectură în fereastra de revizuire (de exemplu 7 zile) | — | Task cu rezultat; fereastra de revizuire |
| Rata de reușită pe acțiune | Acțiuni executate și verificate / acțiuni încercate, pe clasă de efect | `runs`/`operations` (efecte cu aprobare), `agent_steps.status` | Acțiuni pentru toate efectele |
| Eșecuri la verificare | Acțiuni cu verificarea nepotrivită (NeedsReview, hash diferit, citire înapoi diferită) | `runs` NeedsReview (fișiere) | Verificare pentru email, Excel, evidențe |
| Intervenții umane | Numărul evenimentelor de intervenție pe task, pe tipuri | Aprobări ✓, Undo ✓; o corectură a ta pe un câmp scris de agent se poate deduce din `record_changes` | Tabel de intervenții; editare înainte de aprobare; preluare manuală |
| Frecvența aprobărilor | Aprobări cerute pe task; rata de respingere; timpul până la decizie | `approvals` (creată / decisă / consumată) ✓ | Legătura cu task-ul |
| Reîncercări | Reîncercări la furnizor + reîncercări de task (automate sau manuale) | Log (`attempt=`), `inbox_files.attempts` ✓ | Persistate pe task |
| Frecvența rollback | Undo / acțiuni reversibile | `record_changes.undone_at_ms` ✓ | Undo pentru fișiere și Excel; rollback pe clasă |
| Timp economisit | **Estimare:** timpul de referință pe tip de sarcină (dat de tine sau măsurat pe câteva sarcini făcute manual) minus timpul tău de atenție măsurat (aprobări, inspectare, editare, pași manuali) | — | Timpul de referință; măsurarea timpului de atenție în UI |
| Cost LLM/API pe task | Σ tokeni ai mesajelor din task × prețurile introduse de tine, cu reîncercări; nivelul gratuit = 0 bani, dar tokenii se arată | `messages.prompt_tokens/output_tokens` ✓, `model_prices` ✓ | Atribuirea pe task; reîncercările incluse |
| Timpul total până la final | Sfârșit − început, împărțit în agent / om / furnizor / coadă | Timpi pe pași ✓ | Segmente pe task |
| Motivele eșecului | Taxonomie închisă: ProviderUnavailable, QuotaExceeded, BudgetExceeded, InvalidModelOutput, ValidationRefused, VerificationMismatch, PermissionDenied, UserRejected, Timeout, HostInterrupted, OutcomeUnknown | Coduri de eroare parțiale ✓ | Câmp obligatoriu la task |

**Unde se pune:** taskul, costul, taxonomia și intervențiile în completarea M2; auditul în M1, fără redeschidere (folosește evenimentele existente); exportul de diagnostic în M6. Nu e nevoie de un milestone nou.

## I. Decizii de arhitectură de luat acum

Fiecare ar deveni un ADR în arhitectură. Toate păstrează fundația existentă; niciuna nu schimbă stack-ul.

1. **ADR-TASK-001 — un singur model al muncii.** Task → Run → Step → Action pentru orice muncă a agentului; conversația devine un canal. Migrarea: conversațiile existente devin task-uri. Fără asta, B2B-ul cere reconstruirea auditului.
2. **ADR-ACTION-001 — taxonomia efectelor și descriptorul uneltei.**
   - Clase: citire / internă reversibilă / externă reversibilă / externă ireversibilă.
   - Fiecare unealtă declară: clasa, strategia de idempotență, metoda de verificare, metoda de compensare (Undo) și politica implicită de aprobare.
   - Un singur broker. Scrierile în evidențe devin acțiuni interne reversibile, aprobate automat prin politică, dar auditate la fel.
3. **ADR-POLICY-001 — politica de aprobare ca date.**
   - Reguli pe flux: mereu / peste un prag / niciodată (doar pentru efectele interne reversibile).
   - Le evaluează Host-ul. Nici modelul, nici conținutul unui document nu le pot schimba.
   - Aprobări în lot, cu diferențe.
   - Calea de confirmare de încredere (arhitectura §11) devine cerință, nu notă.
4. **ADR-PACK-001 — pachete first-party declarative.**
   - Conțin: scheme (evidențe, extracție), reguli, prompturi, surse de intake, automatizări, permisiuni (foldere, cutii de email, domenii), metrici.
   - Sunt versionate; nu conțin cod.
   - Jobs devine primul pachet. `job_results.rs` devine un motor generic de rezultate, configurat de pachet.
5. **ADR-INTAKE-001 — intake generic.** Surse (folder, email, ulterior API); detecția fișierului stabil; deduplicare după conținut; un task per element; reîncercare cu așteptare; proveniență. Generalizează ce face deja inboxul.
6. **ADR-DATA-001 (există; decizia se aduce înainte):**
   - criptare la repaus (cheie protejată prin DPAPI; dacă trebuie o bibliotecă de criptare SQLite, e o dependență nouă, cu aprobarea ta);
   - retenție pe clasă de document;
   - export și ștergere;
   - auditul cu hash în lanț (ieftin dacă e făcut devreme).
7. **ADR-PROVIDER-002 — clasificarea datelor și politica furnizorului pe flux:**
   - ce câmpuri pot pleca;
   - mascare;
   - fără nivel gratuit pentru date de business;
   - furnizori cu DPA și regiune UE;
   - opțiunea de model local pentru documente sensibile.
8. **ADR-ISO-001 (implementarea se redeschide).** Backendul AppContainer revine din `88ebc89`, pentru parsere, OCR și unelte neîncrezătoare.
9. **ADR-IDENTITY-001 — actori cu identificator stabil:** utilizator, agent, automatizare, canal, în loc de „User/Agent”. Lasă loc separării operator/aprobator fără a construi acum multi-utilizator.
10. **Poziția de livrare:** o instalare = o stație de operator. Fără server, fără administrare centralizată, până la dovezi. Scris explicit, ca proiectul să nu alunece spre SaaS din întâmplare (arhitectura §12 spune deja asta).

## Conflicte explicite și ce parte trebuie să cedeze

1. **„Workeri cu permisiuni limitate” (Core) vs realitate.** Izolarea a fost scoasă; cititorul rulează doar într-un Job Object, care după AGENTS.md nu e un sandbox. **Cedează implementarea:** se readuce backendul probat (M3b). Dacă nu, afirmația trebuie scoasă din teză.
2. **„Acțiuni controlate în browser” în Jobs R1 vs M3** (browserul ultimul, fără izolare, termenii platformelor, nicio ocolire de CAPTCHA). **Cedează strategia:** browserul iese din R1; roadmap-ul rămâne. Emailul dă un efect extern cu consecințe, verificabil curat și mai devreme.
3. **„Rollback unde e posibil tehnic”.** Formularea e corectă, dar trebuie arătată cinstit, pe clase: un email trimis nu se poate retrage, iar un fișier creat nu are azi Undo. **Nu cedează nimic; se precizează** în taxonomia efectelor (ADR-ACTION-001).
4. **Nișa aleasă azi (IT support pentru firme mici) vs pana DocumentOps.** IT support pentru firme străine înseamnă operații pe PC-urile lor: acces de la distanță, drepturi de administrator, mai multe mașini. Asta contrazice „fără administrator implicit” și Host-ul local cu un singur utilizator. DocumentOps folosește aproape numai componente care există deja. **Recomand să cedeze decizia de nișă:** DocumentOps devine pana, IT Ops un pachet ulterior. Decizia e a ta.
5. **„Rutare independentă de furnizor” vs ce e verificat.** Doar Gemini e verificat real, pe un nivel gratuit ai cărui termeni pentru date nu se potrivesc documentelor unor clienți. **Cedează nimic din arhitectură;** se adaugă proba reală a unui furnizor plătit cu DPA ca precondiție a pilotului (M2).
6. **Principiul §3a („fiecare pas adaugă o capacitate încercată în aceeași zi”) vs munca de Core** (modelul task, brokerul unic, telemetria), care nu se vede. **Compromis:** fiecare schimbare de Core vine împreună cu o îmbunătățire vizibilă în Jobs, iar pașii rămân mici.
7. **„Pentru Windows, local” vs așteptările B2B** (administrare centrală, mai mulți utilizatori, aprobare de un manager). **Nu se rezolvă acum.** Pana rămâne pe un singur operator; multi-utilizatorul rămâne un proiect separat (M7, neschimbat).

## J. Riscuri majore

**Tehnice:**

- precizia extracției pe documente reale și calitatea OCR; fără reguli de validare, greșelile ajung în registre;
- costul unificării task/broker: migrări de schemă pe baza ta reală, cu backup și probe, ca până acum;
- fragilitatea browserului și a interfețelor altor aplicații;
- un singur dezvoltator; datoria Jobs scrisă direct în Host;
- dependența de API-uri Windows; testul instabil observat (Credential Manager sub încărcare).

**Securitate:**

- instrucțiuni ascunse în documente și emailuri, procesate în volum: modelul nu trebuie să poată declanșa efecte fără politica Host-ului;
- oboseala aprobărilor („aprob tot”), contracarată prin politici pe risc și aprobări în lot cu diferențe;
- vulnerabilități în parsere, fără izolare;
- tokenii de email (OAuth) în seif;
- scurgeri de date spre furnizorii AI (GDPR, DPA);
- integritatea auditului;
- uneltele MCP cu efect.

**Comerciale:**

- Microsoft are deja automatizare desktop (Power Automate) și agenți în Windows (Copilot Actions). Există și furnizori dedicați de procesare de documente și RPA. Condițiile și prețurile lor actuale *trebuie verificate*; în această sesiune nu le-am verificat;
- adoptarea AI de 5,2% în firmele din România înseamnă vânzări lente;
- răspunderea pentru erori în documente financiare;
- costul suportului pentru firme;
- dovezile dintr-un singur utilizator (n=1) sunt slabe pentru afirmații B2B;
- fereastra limitată a căutării tale de job.

## K. Harta revizuită a milestone-urilor

- **M0 — mediu, workspace, transport demonstrat** → închis (IPC-02 cu alt cont: BLOCKED, nenecesar) → **păstrat neschimbat**.
- **M1 — circuit local complet, persistent, recuperabil** → complet tehnic (UI-01 vizual deschis) → **păstrat neschimbat**; devine fundația brokerului unic, fără redeschidere.
- **M2 — AI real, modele, personalitate, propunerile modelului prin brokerul M1, bugete** → închis funcțional, dar punctul „același broker din M1” e incomplet → **schimbat (completare):** modelul Task unic, brokerul unic cu clase de efect, cost și timp pe task, taxonomia eșecurilor, politica de date pe furnizor; proba reală a unui furnizor plătit cu DPA înaintea unui pilot.
- **M3 — ~~cod~~, PC și browser, cu izolare** → cod scos; izolarea probată și scoasă; PC și browser neîncepute → **schimbat (substanțial, motivat la B):**
  - M3a: operații tipate pe fișiere și Office, cu verificare și rollback;
  - M3b: izolarea readusă pentru parsere, OCR și unelte neîncrezătoare;
  - M3c: browser cu profil dedicat, la final;
  - automatizarea generică mouse/tastatură: amânată după prima versiune.
- **M4 — automatizări durabile și Telegram** → automatizări gata, Telegram neînceput → **schimbat (cerințe inserate):** intake generic (folder, email), conectorul de email (citire, draft, trimitere cu aprobare, verificare), remindere din evidențe; Telegram după email; WhatsApp neschimbat.
- **M5 — memorie, documente, căutare cu proveniență** → Memorie v1, documente și evidențe parțial → **schimbat și adus mai devreme:** extracție structurată cu proveniență, reguli de validare deterministe, clasificare, OCR Windows, retenție; evidențele intră formal aici; embeddings rămân amânate.
- **M6 — calificare pentru utilizare zilnică** → neînceput → **păstrat:** ADR-DATA-001 decis acum și implementat aici; export de audit; precondiție pentru pilotul comercial, nu pentru dogfood.
- **M7 — extindere controlată** → parțial (Three.js, MCP) → **schimbat ușor:** formatul intern de pachete first-party adus înainte, când apare al doilea pachet; SDK pentru terți, subagenți, voce, alte dispozitive, trading rămân amânate; Three.js înghețat.

**Milestone-uri noi: niciunul.** Fiecare capacitate nouă are loc într-un milestone existent.

**Ordinea de execuție propusă** (diferită de numerotare, cum se întâmplă deja în plan):

1. Completarea M2.
2. M5 (extracție, validare) în paralel cu M4 (intake, email).
3. Jobs R1 și evaluarea lui.
4. M3a și M3b.
5. M6.
6. Pilotul DocumentOps.
7. M3c (browser).
8. M7.

## Decizii cerute de la tine

1. Adoptăm harta K în plan (ca actualizare a §3 și a secțiunilor M2–M7, fără renumerotare)?
2. Pana comercială: DocumentOps (recomandat) sau IT Support (ales azi)?
3. Browserul iese din Jobs R1 (aplicare prin email sau manual)?
4. Readucem izolarea probată (ADR-ISO-001) pentru parsere?
5. Deschidem ADR-urile din secțiunea I (TASK, ACTION, POLICY, PACK, INTAKE, PROVIDER-002, IDENTITY), ca documente de decizie, înainte de orice cod?
