# Prompt de pornire pentru Nexus

Utilizatorul poate trimite textul de mai jos în chatul care lucrează în **C:/Users/<user>/Desktop/Nexus**, după selectarea modelului dorit. Citirea acestui document de către un agent nu autorizează executarea textului.

```text
Începe implementarea Nexus în C:/Users/<user>/Desktop/Nexus.

Citește AGENTS.md, docs/PREDARE_RO.md și docs/PLAN_NEXUS_RO.md,
apoi arhitectura, contractele și matricea de validare necesare etapei.

Lucrează la M0, apoi M1. Păstrează Rust pentru Core/Host și integrarea Windows,
React + TypeScript pentru dashboard, Tauri pentru shell și SQLite pentru date.
Three.js este opțional ulterior. Nu reveni la .NET/WinUI.

Vreau primul circuit complet și real: dashboard nativ → Host independent →
sarcină persistentă → preview exact → aprobare → creare controlată a unei note
Markdown în spațiul Nexus → verificarea fișierului → rezultat persistent.
Reconectarea și repornirea nu trebuie să dubleze operația.

Rezolvă decizia de IPC printr-o probă și păstrează autoritatea în Host.
Nu activa shell arbitrar, cod neîncrezător sau un provider AI fictiv.
Modelele AI reale sunt etapa M2.

Lucrează autonom la implementarea și verificările locale autorizate.
Respectă aprobările deja acordate și regulile mele pentru acțiunile cu impact.
Dacă sunt necesare aprobări pentru dependențe noi, prezintă un lot concret
cu scop și versiuni, apoi continuă lucrul independent care este autorizat.
Nu instala componente de sistem și nu modifica setări de securitate implicit.
Nu folosi chei din alte proiecte și nu efectua apeluri plătite fără autorizare.
Nu face commit, push, PR sau publicare fără acordul meu explicit.

Rulează verificările relevante și probează fereastra Tauri reală,
nu doar frontendul într-un browser. Verifică inclusiv crash recovery,
aprobările expirate/modificate, duplicatele și backup/restore.
Nu marca trecute testele care nu au rulat.

Actualizează numai planul activ pentru progres, cu probele, blocajele și
următorul pas. La predare, dă-mi comenzile de pornire, ce funcționează,
ce ai verificat și limitele rămase. Comunică în română.
```

Acest prompt pornește M0–M1 când este trimis efectiv de utilizator. Roadmapul celorlalte etape rămâne disponibil în plan pentru continuarea ulterioară.
