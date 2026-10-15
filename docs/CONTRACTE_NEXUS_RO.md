# Contractele Nexus

Specificație de proiectare pentru M0–M7; nu sunt API-uri deja implementate. [Arhitectură](ARHITECTURA_NEXUS_RO.md) · [Plan](PLAN_NEXUS_RO.md) · [Probe de acceptare](SECURITATE_SI_VALIDARE_RO.md)

## 1. Convenții

Identificatorii sunt tipuri distincte: ProjectId, TaskId, RunId, StepId, ActionId, OperationId, ApprovalId, ArtifactId, EventId și RequestId. Nu folosim interschimbabil șiruri libere pentru aceste concepte.

- Task descrie intenția utilizatorului; Run este o execuție a acelei intenții.
- Action descrie o operație precisă; OperationId identifică efectul logic de-a lungul încercărilor și reconcilierii.
- RequestId corelează transportul. Cheia de idempotency deduplică aceeași cerere logică; nu presupunem că orice retry este o operație nouă.
- Momentele persistente sunt în UTC, iar programările păstrează și fusul orar/semantica locală.
- Duratele din execuție folosesc un ceas monoton unde este necesar.
- Versiunile și reviziile sunt explicite; schimbarea incompatibilă este refuzată, nu interpretată aproximativ.
- Cotele pentru payload, cozi, evenimente, fișiere și rezultate sunt finite, centralizate și testate.
- Limitele inițiale se aleg și se documentează în M0/M1; nu lăsăm valori nelimitate implicite.

Sursa contractelor se alege prin ADR-IPC-001. Tipurile TypeScript se generează din ea; nu întreținem manual două definiții echivalente. Validarea Rust verifică și semantica, nu doar deserializarea.

## 2. Mesajul de comandă și identitatea

O comandă conține versiunea protocolului, RequestId, cheia de idempotency când este necesară, operația, parametrii tipați, proiectul relevant, revizia așteptată și deadline-ul. Actorul efectiv, canalul și granturile se derivă din sesiunea autentificată.

Un câmp de actor furnizat de client poate fi doar o referință validată; nu este dovadă de identitate. Payloadul modelului, un document sau un update Telegram nu poate declara că este administrator.

Serverul refuză înainte de efect:

- operații sau versiuni necunoscute;
- mesaje peste limită și câmpuri interzise de schema unei comenzi;
- scope incompatibil cu sesiunea;
- revizii și precondiții depășite;
- deadline expirat, cote epuizate sau dependențe obligatorii indisponibile.

Metadatele de audit conțin corelare și rezultate redactate. Cheile, tokenurile și conținutul complet al conversațiilor nu se copiază automat în log.

## 3. Entități durabile

| Entitate | Câmpuri și responsabilitate |
| --- | --- |
| Project | Identitate, scope de date/resurse, configurare și revizie |
| Task | Cerere originală, autor, proiect, obiectiv și politică de retenție |
| Run | Încercare, stare, buget, versiuni de policy/persona, timpi și rezultat |
| Step | Unitatea de lucru, dependențe, stare și dovezi |
| Action | Instrument/versiune, argumente validate, resurse, efect și precondiții |
| Approval | Manifestul autorizat, actorul aprobator, expirare și consum/revocare |
| Operation | Intenție persistentă, încercări, executor, efect cunoscut sau incert |
| Artifact | Referință controlată, tip, hash, dimensiune, proveniență și verificare |
| Event | ID și secvență persistente, revizia entității, payload tipat și redus |
| Schedule | Calendar, fus orar, grant limitat, overlap/misfire și revizie |
| MemoryItem | Conținut, sursă, scope, versiune, retenție și status de ștergere |
| ProviderConnection | Endpoint, configurare, capabilități și referință opacă la secret |

Nu stocăm doar textul unei conversații și încercăm să reconstruim din el toate drepturile sau efectele.

## 4. Stări și tranziții

Stările sunt enumuri închise și tranzițiile sunt validate pe server. Finalizarea include dovada rezultatului; un răspuns HTTP sau un exit code izolat nu este întotdeauna suficient.

| Stare Run | Semnificație |
| --- | --- |
| Queued | Acceptat și salvat, fără execuție activă |
| Running | Executorul deține dreptul actual de lucru |
| AwaitingApproval | Așteaptă decizia pentru o acțiune concretă |
| Verifying | Verifică efectul și artefactele |
| CancellationRequested | Oprire cerută; efectele încă pot fi în desfășurare |
| NeedsReconciliation | Există o operație cu efect necunoscut |
| NeedsReview | Reconcilierea automată nu poate decide în siguranță |
| Succeeded | Rezultatul cerut este demonstrat |
| Failed | Eșec cunoscut; efectele și eventualele rezultate parțiale sunt înregistrate |
| Cancelled | Nu mai există execuție activă; efectele deja produse rămân raportate |
| Denied | Acțiunea necesară a fost refuzată și runul nu continuă |

Fluxul obișnuit este Queued → Running → AwaitingApproval → Queued → Running → Verifying → Succeeded. Pașii fără aprobare pot omite starea de așteptare. Nu reluăm un worker adormit presupunând că aprobarea veche este încă valabilă.

La startup, un Run aflat anterior în execuție nu devine automat Queued. Hostul inspectează operațiile sale: citirile pot fi reprogramate conform politicii; mutațiile incerte intră în NeedsReconciliation.

O Action trece prin Prepared, AwaitingApproval dacă este necesar, Authorized, Executing și Verified/Failed/NeedsReconciliation. Refuzul, expirarea și invalidarea sunt rezultate explicite, nu erori generice care declanșează retry.

## 5. Aprobare și execuție

Manifestul aprobării include:

- actor, proiect și canal permis;
- OperationId, instrument și versiune;
- parametrii normalizați și resursele exacte;
- tipul efectului și destinația datelor;
- revizia policy, precondiții relevante și limite;
- expirare, caracter de unică folosință sau grant limitat explicit;
- hashul manifestului produs prin serializarea canonică aleasă.

Preview-ul este generat din același manifest verificat de Host. Frontendul nu trimite un simplu „approved=true” pe care executorul să îl creadă.

Originea deciziei este verificată separat de validitatea manifestului. Pentru acțiunile sensibile din M3 se cere confirmarea prin suprafața de încredere aleasă; rendererul de conținut extern nu poate acorda singur această autoritate. M1 limitează operațiile la artefactele Nexus și nu pretinde că a demonstrat acest prag ulterior.

Înaintea efectului, Hostul reverifică drepturile actuale, revocarea și precondițiile. Într-o tranzacție consumă aprobarea pentru OperationId și salvează intenția/claimul executorului. Consumul nu autorizează automat orice parametri ai unui retry ulterior.

Tranzacția SQLite nu este atomică împreună cu un efect Windows sau de rețea. După o cădere în acel interval, aceeași operație se reconciliază. Expirarea aprobării în timpul unei operații deja începute nu dovedește că efectul a fost oprit; se verifică rezultatul.

Pentru modificări sensibile de resurse, precondițiile se verifică cât mai aproape de operație, ideal pe handle-ul efectiv. O verificare a căii urmată de accesul nesigur la aceeași cale poate permite înlocuirea resursei între pași.

## 6. Definiția unui instrument

ToolDescriptor descrie ToolId, versiune, schemele intrării/ieșirii, efectele posibile, politica de date, permisiunile minime, limitele, anularea și verificarea.

Semantica de retry este declarată de implementarea de încredere:

- ReadOnly: fără efect intenționat asupra resurselor; retry limitat dacă politica permite.
- IdempotentWithKey: destinația susține deduplicarea pentru o cheie definită.
- Reconcilable: verificăm resursa/operația înainte de a decide reluarea.
- NonRepeatable: nu reluăm automat după un efect incert.

Descriptorul include procedura de reconciliere, dovezile de succes și eșec, plus compensarea dacă există. Compensarea este o operație nouă autorizată; nu promitem rollback universal.

RiskLevel este o indicație pentru policy, nu o permisiune. Modelul nu își poate reclasifica singur acțiunea ca fiind sigură.

## 7. Acțiunea M1: CreateMarkdownArtifact

Instrumentul creează o notă nouă în Artifact Store administrat de Nexus. Interfața arată titlul, conținutul exact și destinația rezolvată înaintea aprobării.

- Hostul alege calea din profil/proiect/OperationId; clientul nu furnizează o cale arbitrară.
- Textul este UTF-8 și limitat explicit; propunere inițială: maximum 1 MiB, confirmată în configurarea M1.
- Schema nu acceptă execuție, download sau instrucțiuni shell.
- Fișierul se creează numai dacă nu există; nu suprascriem un fișier găsit.
- Se verifică rădăcina reală și se refuză reparse points/junctions sau alte redirecționări incompatibile cu scope-ul.
- Intenția păstrează OperationId, conținutul/hashul așteptat și versiunea algoritmului de creare/verificare.
- Succesul cere fișierul așteptat, conținutul și hashul verificate, plus manifestul Artifact salvat.
- Deschiderea rezultatului folosește ArtifactId validat și un vizualizator permis; nu un URL sau o comandă arbitrară din output.

Strategia de scriere tratează explicit fișierul parțial: etapizare într-o cale Nexus atribuită operației, flush după cerința de durabilitate și publicare fără înlocuire sau o alternativă demonstrată. Căderea între oricare doi pași are un caz de recuperare.

La reluare:

1. Dacă nu există efect, policy decide dacă operația poate fi continuată cu același OperationId.
2. Dacă există dovadă a fișierului complet al aceleiași operații, rezultatul se reconciliază fără o a doua creare.
3. Dacă fișierul este parțial, modificat, ocupat sau atribuirea este incertă, nu îl înlocuim; marcăm cazul pentru reconciliere/review.

Un hash identic într-o locație arbitrară nu este suficient pentru a revendica un fișier ca fiind creat de Nexus.

## 8. Suprafața de comenzi locală

Numele de mai jos sunt propuneri de contract, nu rute deja disponibile.

| Comandă | Scop | Etapă |
| --- | --- | --- |
| Handshake / GetHealth | Versiuni, identitate, disponibilitate, fără secrete | M0 |
| GetSnapshot / SubscribeEvents | Stare consistentă și continuare de la cursor | M0–M1 |
| SubmitTask / GetRun | Cerere persistentă și stare autorizată | M1 |
| DecideApproval / RevokeGrant | Decizie legată de manifest și drepturi revocabile | M1 |
| CancelRun / StopHost | Anulare sau oprire controlată | M1 |
| GetArtifact / OpenArtifact | Metadate, citire/deschidere autorizată | M1 |
| CreateBackup / RestoreToNewProfile | Backup coerent și restaurare inactivă | M1 |
| RegisterProject / UpdateProjectScope | Adăugare explicită și schimbare de scope | M2–M3 |
| ListProviders / SaveProviderConnection | Configurație redactată; secret separat | M2 |
| SaveProviderSecret / RemoveProviderSecret | Scriere în seif; valoarea nu se returnează | M2 |
| GetPersona / SavePersona | Profil versionat cu validare | M2 |
| CreateSchedule / DisableSchedule | Grant, calendar și revocare | M4 |

Nu expunem ExecuteSql, RunAnyCommand sau GetAllSecrets. Dialogul de introducere a unei chei poate transmite secretul o singură dată prin canalul autorizat; UI nu îl persistă, nu îl recitește din seif și nu îl atașează telemetriei.

## 9. Erori și retry

Codurile includ InvalidRequest, UnsupportedVersion, Unauthenticated, Forbidden, ApprovalRequired, ApprovalExpired, ApprovalInvalidated, RevisionConflict, ResourceChanged, NotFound, Busy, QuotaExceeded, DeadlineExceeded, ProviderUnavailable, Cancelled, OutcomeUnknown și InternalFailure.

Eroarea are un mesaj potrivit utilizatorului, cod stabil, RequestId/OperationId când există, caracter temporar declarat și informația necesară remedierii. Detaliile interne se redactează.

Caracterul temporar nu autorizează retry automat al unei mutații. Decizia folosește și starea operației, semantica instrumentului, bugetul și politica actuală.

O cheie de idempotency refolosită cu alți parametri produce conflict. Aceeași cheie cu aceiași parametri întoarce aceeași operație/stare; nu creează o a doua acțiune.

## 10. Evenimente și concurență

Fiecare eveniment durabil are EventId, secvență monotonă per profil, tip/versiune, entitate și revizie. Cursorul este opac pentru client.

Snapshotul și watermarkul sunt coerente. După reconectare, clientul aplică evenimentele ulterioare; duplicatele nu dublează activitatea. Dacă retenția a eliminat cursorul, clientul primește cerere explicită de refresh.

Delta de streaming AI sau progresul foarte frecvent pot fi efemere și limitate. Mesajul final, starea taskului, aprobările și efectele sunt persistente. Reconectarea nu pretinde recuperarea unui token efemer care nu a fost salvat.

Evenimentele sunt notificări, nu comenzi de execuție. Un consumator nu pornește un efect doar pentru că a văzut de două ori un eveniment de status.

M1 poate avea un singur executor. Înainte de paralelizare, adăugăm claims/leases și tokenuri de generație pentru respingerea rezultatelor executorilor depășiți. Un fencing token în SQLite nu oprește magic un proces extern încă activ; înainte de reasignare se verifică oprirea sau se reconciliază efectul.

## 11. Contractul furnizorului AI

ProviderRequest conține modelul explicit, versiunea profilului, instrucțiuni autorizate, conversația permisă, referințe de atașament filtrate, descriptorii instrumentelor, buget și deadline.

ProviderEvent distinge text vizibil, propuneri de instrumente, date de utilizare, eroare, anulare și motiv de finalizare. Argumentele propuse se validează înainte de afișarea aprobării sau execuție.

- Selectarea modelului verifică capabilitățile; o funcție nesuportată este refuzată sau adaptată explicit.
- Tokenurile/ID-urile de continuare specifice furnizorului rămân la acel adaptor.
- Fallbackul sau schimbarea modelului reconstruiește contextul permis și reverifică egress și buget.
- Timeoutul unui apel nu este dovadă că furnizorul nu l-a procesat/facturat.
- Retries și apelurile copil consumă bugetul comun; estimările se reconciliază cu utilizarea raportată.
- Testele cu server controlat verifică protocolul. Apelurile reale sunt o probă separată, autorizată.

## 12. Module, automatizări și canale

ModuleManifest include ID, versiune, compatibilitate Host/protocol, emitent/proveniență, instrumente, scope, tip de executor și cerințe. Înregistrarea nu acordă permisiuni automat. Dezactivarea revocă viitoarele execuții și tratează explicit cele active.

O extensie nu primește implicit handle SQLite, acces la seif sau ocolirea brokerului. Migrarea datelor unei extensii are versiune, backup și aprobare când afectează date existente.

Schedule păstrează calendarul, fusul orar, regula pentru ora ambiguă/inexistentă, comportamentul la missed run, overlap, retry și grantul limitat. Fiecare declanșare creează o cerere deduplicabilă și reevaluează drepturile.

Telegram păstrează identitatea numerică asociată și ultimul update procesat tranzacțional. Callbackurile de aprobare sunt legate de actor, mesaj/acțiune și expirare; textele copiate sau redirecționate nu sunt aprobări.

Pentru execuții copil, scope-ul este un subset al celui părinte și costul intră în bugetul comun. Delegarea nu generează un nou nivel de autoritate.

## 13. Versionare și compatibilitate

Protocolul, schema SQLite, manifestele modulelor, policy și descriptorii instrumentelor au versiuni separate. Handshake-ul declară compatibilitatea înainte de comenzi.

Upgradeul oprește admisia de sarcini, tratează operațiile active, creează backupul necesar și verifică schema. Nu pornim un Host vechi peste o schemă mai nouă presupunând compatibilitate. Downgrade-ul este suportat doar printr-o procedură verificată.

Schimbările contractelor actualizează generarea tipurilor, testele consumatorilor și cazurile afectate din matrice. Un test care reproduce aceeași implementare greșită nu înlocuiește proba unei proprietăți: efect unic demonstrabil, scope respectat, rezultat recuperabil.
