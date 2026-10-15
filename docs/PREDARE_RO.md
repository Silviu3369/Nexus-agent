# Predare Nexus

Snapshot datat: **2026-10-13**, ora Europei Centrale. Workspace: **C:/Users/<user>/Desktop/Nexus**.

Acest document permite preluarea proiectului fără reconstruirea conversației. Nu schimbă modelul, nu creează alt chat și nu autorizează singur implementarea.

## 1. Cererea și direcția

Utilizatorul vrea un agent personal Windows numit Nexus, cu dashboard, furnizori AI selectabili, personalitate proprie și răspunsuri în limba interlocutorului. Îl va folosi ca asistent, agent de programare și agent pentru PC. Telegram, automatizările, memoria și extensiile sunt în roadmap.

Direcția este Rust pentru Core/Host/PC/workers, React + TypeScript pentru dashboard, Tauri pentru shell, SQLite pentru persistență și Three.js opțional pentru reprezentarea vizuală. Python și PowerShell au roluri strict justificate.

Agentul de cod activ în acest workspace execută munca de dezvoltare. Identitatea agentului nu este o decizie de a fixa un model ca furnizor al Nexus și nu justifică inventarea unui identificator API.

## 2. Citește la preluare

1. [AGENTS.md](../AGENTS.md) — reguli locale și limitele autorizației.
2. [Planul activ](PLAN_NEXUS_RO.md) — starea curentă, etapa și criteriile.
3. [Arhitectura](ARHITECTURA_NEXUS_RO.md) — granițe, layout și deciziile rămase.
4. Pentru implementarea M1: [contractele](CONTRACTE_NEXUS_RO.md) și [matricea de validare](SECURITATE_SI_VALIDARE_RO.md).

Nu reciti tot pachetul pentru fiecare editare minoră. Planul este unicul registru de progres; actualizează-l la predare și la închiderea etapelor.

AGENTS.md păstrează regulile de lucru concise și trimite către specificații. Descoperirea și prioritatea instrucțiunilor urmează mecanismul documentat al produsului Codex. [Ghid oficial AGENTS.md](https://learn.chatgpt.com/docs/agent-configuration/agents-md)

## 3. Starea verificată în acest snapshot

Înaintea scrierii documentelor, directorul Nexus era gol, verificat prin inventar local. Vechile șabloane .NET observate anterior în conversație nu mai erau prezente. Nu există dovadă aici despre cine le-a eliminat; nu presupune cod de migrat sau date de recuperat.

Pachetul creat conține README, AGENTS și șase documente în docs. Nu au fost create manifeste Cargo/pnpm, executabile, teste de produs sau un repository Git. Nu există rezultat de build sau o aplicație nativă validată.

| Instrument / componentă | Observație din verificarea locală | Ce nu demonstrează |
| --- | --- | --- |
| rustc | 1.97.1, host x86_64-pc-windows-msvc | Compatibilitatea viitoarelor dependențe |
| cargo | 1.97.1 | Un build Nexus reușit |
| Node.js | v24.19.0 | Instalarea frontendului |
| pnpm.cmd | 11.25.0; comandă executată cu succes | Reproductibilitatea unui lockfile încă inexistent |
| npm / npm.cmd | Nu au fost găsite în PATH | Nu blochează automat un proiect care folosește pnpm |
| MSVC | vswhere a identificat componenta VC.Tools.x86.x64 în Build Tools 2022 | Linkarea aplicației Tauri |
| Windows SDK | Director Include 10.0.26100.0 prezent | Compatibilitatea întregului build |
| WebView2 | Directorul Application conține 154.0.4258.62 | Pornirea efectivă și starea de înregistrare a runtime-ului |

Calea pnpm observată: C:/Users/<user>/.cache/codex-runtimes/codex-primary-runtime/dependencies/bin/fallback/pnpm.cmd. Nu fixa această cale în produs; verific-o în mediul de lucru. Build Tools: C:/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools.

Versiunile sunt observații, nu cerințe de upgrade și nici lockfile. Verifică actualitatea lor înainte de bootstrap și fixează combinația compatibilă aleasă.

## 4. Configurarea WinUI abandonată

O încercare anterioară de configurare pentru WinUI, care ar fi instalat Visual Studio Community 2026 și activat Developer Mode, a fost respinsă de mecanismul automat de aprobare înaintea execuției. Motivul raportat a fost lipsa aprobării explicite pentru acele schimbări de sistem. Nu reprezintă o instalare efectuată.

Acea configurare nu mai aparține stackului Nexus. Nu o relansa și nu încerca altă comandă pentru a ocoli respingerea. Verifică cerințele Rust/Tauri pe componente; pentru Windows, documentația Tauri indică toolchainul C++ și WebView2. [Prerechizite Tauri](https://v2.tauri.app/start/prerequisites/)

Dacă apare o lipsă reală, explică exact componenta și schimbarea necesară. Nu instala întregul mediu WinUI ca soluție implicită.

## 5. Autorizație și autonomie

La momentul predării, cererea activă este pregătirea planului și a documentelor. Instrucțiunea anterioară de a nu scrie cod până la lămurirea stackului nu este tratată ca fiind anulată doar prin citirea acestui fișier.

Utilizatorul poate porni implementarea printr-o cerere explicită sau prin trimiterea [promptului pregătit](PROMPT_START_RO.md). Odată autorizată etapa, lucrează autonom la inspecție, editări locale și verificări obișnuite în acel scope.

Respectă aprobările deja acordate. Pentru o dependență de producție nouă, schimbări de sistem, migrarea datelor existente, credențiale, costuri, publicare sau Git write, aplică regulile umane curente. Prezintă un lot concret când o aprobare este necesară; nu întreba pentru fiecare comandă de citire sau test obișnuit.

Nu căuta chei în alte proiecte. Nu contacta servicii plătite pentru a demonstra un flux M1 care funcționează offline. Nu comite, nu publica și nu creea alt chat fără cererea explicită.

## 6. Primul lucru de construit după pornire

Ținta inițială este M0 + M1, în această ordine:

1. Reinspectează workspace-ul și instrumentele; pregătește dependențele necesare și verifică aprobările.
2. Bootstrap Cargo/pnpm/Tauri și dovedește Hostul independent, versiunea protocolului și conexiunea locală autentificată.
3. Închide ADR-IPC-001 pe baza unei probe. Nu activa execuție arbitrară de cod cât timp ADR-ISO-001 este deschis.
4. Construiește circuitul real: creare sarcină → preview exact pentru o notă → aprobare → creare controlată → verificare → rezultat persistent.
5. Probează reconnect, duplicate, refuz/expirare, cădere după efect, backup/restore și fereastra Tauri reală.
6. Actualizează planul cu dovezi, limite și comenzi reproductibile.

Nu începe cu zeci de module goale, un provider AI fictiv sau un dashboard care afișează activitate simulată. Prima interfață trebuie să permită fluxul real al M1. Chatul AI intră la M2, prin același sistem de permisiuni.

## 7. Decizii care nu se redeschid fără motiv

- Numele Nexus și stackul Rust/React/Tauri.
- Host independent de închiderea UI.
- Autoritatea în Host; modelul propune, policy și brokerul decid execuția.
- SQLite autoritativ, intenție înainte de efect și reconciliere.
- Personalitate, memorie și permisiuni separate.
- Fără drepturi admin implicite, fallback AI tacit sau shell arbitrar în frontend.

Transportul exact, mecanismul său de autentificare și generarea DTO-urilor cer proba M0. Backendul de izolare și modelul de criptare au pragurile documentate în arhitectură. Sunt puncte de decizie controlate, nu permisiune pentru schimbarea stackului.

## 8. Raportarea către utilizator

Comunică în română. Spune concret ce funcționează, cum a fost verificat și ce rămâne neverificat. Distinge surse, build, executabil pornit și pachet instalat.

Dacă o verificare este blocată, indică acțiunea, cauza și remediul minim. Continuă activitatea independentă autorizată. Nu raporta siguranță, viteză, spațiu pe disc sau compatibilitate pe baza unor estimări prezentate ca măsurători.

La finalul unei etape, utilizatorul trebuie să poată porni rezultatul și reproduce fluxul promis. Publicarea rămâne o decizie separată.
