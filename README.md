# Mediagenerator

Intern selvbetjeningsside for Oslofjord IT. Ansatte logger inn med bedriftens SSO
(Microsoft Entra ID) og genererer bilder, lydklipp eller korte videoer ved å skrive
en prompt. Genereringen skjer mot Microsoft Azure AI Foundry / Azure OpenAI.

Hele applikasjonen er skrevet i Rust.

> **Status:** Fase 4 av 6 er ferdig. Se [Leveranseplan](#leveranseplan).

---

## Innhold

- [Teknologivalg](#teknologivalg)
- [Prosjektstruktur](#prosjektstruktur)
- [Kom i gang lokalt](#kom-i-gang-lokalt)
- [Innlogging](#innlogging)
- [Grensesnittet](#grensesnittet)
- [Jobbmodellen](#jobbmodellen)
- [Miljøvariabler](#miljøvariabler)
- [Entra ID – app-registrering steg for steg](#entra-id--app-registrering-steg-for-steg)
- [Azure-ressurser som må opprettes](#azure-ressurser-som-må-opprettes)
- [Docker](#docker)
- [Kvalitetsporter](#kvalitetsporter)
- [Dokumenterte valg](#dokumenterte-valg)
- [Leveranseplan](#leveranseplan)

---

## Teknologivalg

| Område | Valg |
| --- | --- |
| Språk | Rust stable (utviklet og testet på 1.98.1), edition 2024 |
| Web | axum 0.8 på tokio |
| Middleware | tower + tower-http (compression, trace, cors, limits, ServeDir, set-header, timeout) |
| Templating | askama 0.16 (typed templates, server-side rendering) |
| Frontend | HTMX 2 + ett lite vanilla-JS-skript. Ingen React, ingen Node i runtime |
| Styling | Tailwind CSS v4 via standalone CLI-binær (ingen npm) |
| Database | PostgreSQL via sqlx (Azure Database for PostgreSQL Flexible Server) |
| Auth | OpenID Connect mot Microsoft Entra ID (`openidconnect` + `oauth2`) |
| Sesjoner | tower-sessions med PostgreSQL-store, HttpOnly + Secure + SameSite=Lax |
| HTTP-klient | reqwest med rustls-tls |
| Serialisering | serde / serde_json |
| Feilhåndtering | `thiserror` i biblioteker, `anyhow` i binær, `AppError: IntoResponse` |
| Logging | tracing + tracing-subscriber, JSON-output i produksjon |
| Config | figment, lest fra miljøvariabler (12-factor) |
| Container | Multi-stage Dockerfile med cargo-chef, debian-slim, non-root |

### Vurdert, men ikke implementert nå

**Leptos SSR** som full Rust/WASM-frontend. HTML-strukturen holdes derfor ren og
komponentorientert (én askama-partial per UI-komponent, ingen forretningslogikk i
templates), slik at komponentene kan oversettes til Leptos-komponenter senere uten
å bygge om markup-et.

---

## Prosjektstruktur

```
mediagenerator/
├─ Cargo.toml                 workspace
├─ rust-toolchain.toml        pinner stable + rustfmt/clippy
├─ crates/
│  ├─ web/                    axum-server, ruter, templates, middleware, config
│  ├─ auth/                   OIDC/Entra ID, sesjon, require_auth, rollesjekk
│  ├─ providers/              Azure AI Foundry-klienter: image, audio, video
│  ├─ domain/                 modeller, jobbtilstander, validering, feiltyper, i18n
│  └─ storage/                sqlx-repository + Azure Blob Storage
├─ migrations/                sqlx-migrasjoner
├─ templates/                 askama .html
├─ assets/                    css, js, ikoner, logo
├─ .env.example
├─ Dockerfile
└─ README.md
```

Avhengighetsretningen er enveis: `domain` kjenner ingen andre crates;
`storage`, `auth` og `providers` bygger på `domain`; `web` binder alt sammen.

---

## Kom i gang lokalt

### Forutsetninger

- Rust stable (`rustup toolchain install stable`)
- På Windows: Visual Studio Build Tools med «Desktop development with C++»
  (MSVC-linker og Windows SDK)
- PostgreSQL 15+ – eller bruk utviklingsdatabasen i Azure, se [Azure-ressurser](#azure-ressurser-som-må-opprettes)
- Tailwind standalone CLI – se [Bygg CSS](#bygg-css). Ingen npm.

### Oppsett

```bash
cp .env.example .env
# Fyll inn verdiene – se tabellen under.

# Generer en sesjonsnøkkel (minst 64 tegn):
openssl rand -base64 64
```

### Kjør

```bash
cargo run --bin mediagenerator
# -> http://localhost:8080/health
```

Serveren nekter å starte hvis en påkrevd miljøvariabel mangler eller er ugyldig.
Det er tilsiktet: prosessen skal aldri kjøre halvkonfigurert.

### Verifiser

```bash
curl -i http://localhost:8080/health
curl -i http://localhost:8080/ready
```

Åpne deretter <http://localhost:8080> i en nettleser. Du blir sendt til Entra ID,
og etter innlogging tilbake til forsiden.

### Bygg CSS

Tailwind kjøres som frittstående binær. Ingen npm, ingen `node_modules`.

```bash
# Hent binæren én gang (Windows-eksempel; bytt asset-navn for Linux/macOS):
curl -sL -o tailwindcss.exe \
  https://github.com/tailwindlabs/tailwindcss/releases/latest/download/tailwindcss-windows-x64.exe

# Bygg:
./tailwindcss -i assets/css/input.css -o assets/css/app.css --minify

# Eller under utvikling:
./tailwindcss -i assets/css/input.css -o assets/css/app.css --watch
```

`assets/css/app.css` er sjekket inn. Det betyr at container-imaget ikke trenger
noe CSS-verktøy, og at `cargo run` virker rett etter en `git clone`. Husk å
bygge på nytt og committe når du endrer klasser i en template.

### Uten PostgreSQL lokalt

Sett `SESSION_STORE=memory` i `.env`. Da kjører sesjonene i minnet og du trenger
ingen database for å teste innlogging. Verdien avvises når `APP_ENV=production`.

## Innlogging

Autentiseringen er OpenID Connect Authorization Code Flow med PKCE mot Entra ID.

| Rute | Beskrivelse |
| --- | --- |
| `GET /auth/login` | Starter innloggingen. `?return_to=/sti` tas vare på |
| `GET /auth/callback` | Tar imot omdirigeringen fra Entra ID |
| `POST /auth/logout` | Tømmer sesjonen og avslutter den hos Entra ID |

Alt hemmelig — PKCE-verifier, `state`, `nonce` og ID-tokenet — ligger
server-side i sesjonen. Nettleseren holder bare en signert cookie med en
ugjennomsiktig sesjons-ID.

Det som valideres på ID-tokenet: signatur mot JWKS (hentet via discovery og
cachet i 12 timer), issuer, audience, nonce, `exp`, og `at_hash` mot access
token. Sesjons-ID-en byttes ut etter innlogging, så en sesjon en angriper har
plantet på forhånd ikke kan brukes etterpå.

`require_auth` beskytter alt utenom `/health`, `/ready`, `/assets/*` og
`/auth/*`. Avvisninger tilpasses kallet: JSON på `/api/*`, `HX-Redirect` på
HTMX-kall, og vanlig omdirigering ved sidenavigasjon.

`require_role` er på når `REQUIRED_APP_ROLE` har en verdi, og av når den er tom.
Rollen godtas både som app-rolle og som gruppemedlemskap, slik at tilgang kan
styres på begge måter uten kodeendring.

## Grensesnittet

Server-side rendering med askama, HTMX for interaktivitet, og ett lite
vanilla-JS-skript. Ingen React, ingen Node i runtime.

| Rute | Innhold |
| --- | --- |
| `GET /` | Generatoren: hero, promptfelt, medietypevalg, forslag, «Generert» |
| `GET /historikk` | Brukerens egne genereringer (fylles i fase 6) |
| `GET /eksempler` | Arkiv over generert innhold (fylles i fase 6) |
| `POST /api/generate` | Tar imot en generering og svarer med resultatkortet |
| `GET /api/me` | Innlogget bruker som JSON |

Komponentene ligger som én fil hver under [templates/partials/](templates/partials/)
uten logikk i seg. Det er det som holder Leptos-alternativet åpent: komponentene
kan porteres uten at markup må skrives om.

**Uten JavaScript** fungerer fortsatt innlogging, utlogging, navigasjon,
skjemainnsending og bytte av forslagsrad. Det JS legger til er tegntelleren,
deaktivert knapp ved tom prompt, forslag som fyller feltet, varselet nederst til
høyre og «Kopier lenke».

**Tilgjengelighet:** `lang="nb"`, hopp-til-innhold-lenke, synlig fokusmarkering
overalt, native radioknapper (så piltastnavigasjon og opplesing virker som
plattformen gjør det), `aria-live` på varsler, og `prefers-reduced-motion`
respekteres. Kontrasten på dempet tekst mot bakgrunn er ca. 6,3:1 og på blå
knapp med hvit tekst ca. 5,6:1 — begge over AA-kravet på 4,5:1.

## Jobbmodellen

Generering er asynkron hele veien. `POST /api/generate` validerer, skriver en rad
og svarer med én gang; en liten arbeiderpool driver jobben videre, og nettleseren
følger med over Server-Sent Events.

```
POST /api/generate  ->  jobs-rad (queued)  ->  kø  ->  arbeider (running)
                                                          |
                          SSE: /api/jobs/{id}/events  <----+
                                                          v
                                                    succeeded / failed
```

| Rute | Innhold |
| --- | --- |
| `POST /api/generate` | Oppretter jobben og svarer med kortet for `queued` |
| `GET /api/jobs/{id}` | Status som JSON |
| `GET /api/jobs/{id}/events` | SSE-strøm som lukkes på terminal status |
| `GET /api/jobs/{id}/card` | Kortet som HTML-fragment |

`JobStatus` er `Queued | Running | Succeeded | Failed { code, message } |
Cancelled`. Feilkoden er stabil og lagres i `jobs.error_code`; meldingen er
norsk og kommer fra i18n-modulen.

**Du kan navigere vekk eller oppdatere siden.** Jobben ligger i databasen, ikke i
nettleseren. Forsiden spør `active_for_user` ved hver innlasting og rendrer
kortet for det som fortsatt kjører, og JS-en kobler seg på strømmen igjen. Ingen
`localStorage`, ingenting å miste.

**Ved omstart** feiles alt som sto igjen som `queued` eller `running`, med en
egen melding. En rad som henger på «Genererer …» for alltid er verre enn en som
sier at den ble avbrutt og kan prøves på nytt.

**Eierskap er en `where`-betingelse**, ikke en sjekk kalleren må huske. En jobb
som tilhører noen andre gir «finnes ikke», ikke «ingen tilgang» — at raden
finnes er i seg selv informasjon.

### Datamodell

`users`, `jobs`, `assets` og `audit_log`, definert i
[migrations/0001_initial.sql](migrations/0001_initial.sql). Migrasjonene er
kompilert inn i binæren og kjøres ved oppstart, så container-imaget migrerer seg
selv uten et eget artefakt å holde i takt.

To databasebeskrankninger er verdt å merke seg: `jobs.status` har en
check-constraint på de fem gyldige verdiene, og `jobs_error_matches_status`
krever at en jobb har feilkode hvis og bare hvis den er `failed`.

---

## Miljøvariabler

Alle innstillinger leses fra miljøet. Se [.env.example](.env.example) for malen.

| Variabel | Påkrevd | Default | Beskrivelse |
| --- | --- | --- | --- |
| `APP_BASE_URL` | ja | – | Offentlig base-URL, uten skråstrek til slutt |
| `APP_PORT` | nei | `8080` | TCP-port |
| `APP_ENV` | nei | `development` | `development` eller `production` |
| `RUST_LOG` | nei | `info` | tracing-filter |
| `ASSETS_DIR` | nei | `assets` | Katalog som serveres på `/assets` |
| `DATABASE_URL` | ja | – | PostgreSQL connection string |
| `SESSION_SECRET` | ja | – | Nøkkel for cookie-signering, minst 64 tegn |
| `SESSION_STORE` | nei | `postgres` | `postgres` eller `memory`; `memory` avvises i produksjon |
| `AZURE_TENANT_ID` | ja | – | Entra ID tenant |
| `AZURE_CLIENT_ID` | ja | – | Entra ID app (client) ID |
| `AZURE_CLIENT_SECRET` | nei | – | Utelates ved Managed Identity |
| `OIDC_REDIRECT_URI` | ja | – | Må matche app-registreringen eksakt |
| `REQUIRED_APP_ROLE` | nei | – | Tom verdi slår av rollesjekken |
| `AZURE_OPENAI_ENDPOINT` | ja | – | Uten skråstrek til slutt |
| `AZURE_OPENAI_API_VERSION` | ja | – | Bevisst uten default, se [Dokumenterte valg](#dokumenterte-valg) |
| `IMAGE_DEPLOYMENT` | ja | – | Deployment-navn for bilde |
| `AUDIO_DEPLOYMENT` | ja | – | Deployment-navn for tale |
| `VIDEO_DEPLOYMENT` | ja | – | Deployment-navn for video (Sora) |
| `AZURE_OPENAI_API_KEY` | nei | – | Kun lokal utvikling; avvises når `APP_ENV=production` |
| `AZURE_STORAGE_ACCOUNT` | ja | – | Lagringskonto for generert media |
| `AZURE_STORAGE_CONTAINER` | nei | `media` | Privat container |
| `SAS_TTL_MINUTES` | nei | `60` | Levetid på SAS-lenker |
| `MAX_PROMPT_CHARS` | nei | `4000` | Maks promptlengde |
| `RATE_LIMIT_PER_HOUR` | nei | `20` | Genereringer per bruker per time |
| `RETENTION_DAYS` | nei | `90` | Oppbevaringstid for prompts og media |

`APP_ENV`, `ASSETS_DIR` og `SESSION_STORE` er tillegg til listen i
spesifikasjonen. De dekker henholdsvis JSON-logging/HSTS, at Docker-imaget skal
finne statiske filer uavhengig av arbeidskatalog, og lokal utvikling uten
PostgreSQL.

---

## Entra ID – app-registrering steg for steg

1. **Opprett registreringen.** Entra-portalen -> *App registrations* -> *New
   registration*. Navn: `Mediagenerator`. *Supported account types*: «Accounts in
   this organizational directory only (Single tenant)».
2. **Redirect URI.** Plattform *Web*, URI `https://<ditt-domene>/auth/callback`.
   Legg til `http://localhost:8080/auth/callback` som ekstra URI for lokal
   utvikling. Verdien må være identisk med `OIDC_REDIRECT_URI`.
3. **Front-channel logout URL.** `https://<ditt-domene>/auth/logout`.
4. **Noter ID-ene.** *Overview* -> *Application (client) ID* gir `AZURE_CLIENT_ID`,
   *Directory (tenant) ID* gir `AZURE_TENANT_ID`.
5. **Client secret (kun lokal utvikling).** *Certificates & secrets* -> *New
   client secret*. Kopier verdien til `AZURE_CLIENT_SECRET`. I Azure brukes
   Managed Identity i stedet, og hemmeligheten skal ikke finnes.
6. **API-tillatelser.** *API permissions* -> Microsoft Graph -> *Delegated* ->
   `openid`, `profile`, `email`, `User.Read`. Klikk *Grant admin consent*.
7. **App-rolle.** *App roles* -> *Create app role*:
   - Display name: `Mediagenerator-bruker`
   - Allowed member types: *Users/Groups*
   - Value: `Mediagenerator.User` — settes som `REQUIRED_APP_ROLE`
   - Description: «Kan generere media i Mediagenerator»
8. **Tildel rollen.** Entra -> *Enterprise applications* -> `Mediagenerator` ->
   *Users and groups* -> *Add user/group* -> velg gruppe -> rolle
   `Mediagenerator-bruker`.
9. **Token-konfigurasjon.** *Token configuration* -> *Add optional claim* -> ID ->
   `upn` og `email`. Hvis dere heller vil styre tilgang på gruppemedlemskap:
   *Add groups claim* -> *Security groups*.

Discovery-dokumentet applikasjonen leser er
`https://login.microsoftonline.com/{TENANT_ID}/v2.0/.well-known/openid-configuration`.

---

## Azure-ressurser som må opprettes

| Ressurs | Formål | Merknad |
| --- | --- | --- |
| Resource group | Samler alt | f.eks. `rg-mediagenerator-prod` |
| Azure AI Foundry / Azure OpenAI | Bilde-, lyd- og videogenerering | Krever deployments for bilde, tale og Sora |
| Azure Database for PostgreSQL Flexible Server | Jobber, brukere, audit-logg, sesjoner | Private endpoint anbefales |
| Azure Storage Account (StorageV2) | Generert media | **Privat** container, ingen anonym tilgang |
| Azure Key Vault | `SESSION_SECRET`, DB-passord | Refereres fra Container Apps-secrets |
| Azure Container Registry | Container-image | |
| Azure Container Apps (+ environment) | Kjøring | Systemtildelt Managed Identity |
| Log Analytics workspace | Logger | Container Apps skriver JSON-logger hit |

### Opprettet så langt

Utviklingsmiljøet står i abonnementet **Oslofjord IT Drift**:

| Ressurs | Verdi |
| --- | --- |
| Resource group | `rg-mediagenerator-dev` (norwayeast) |
| PostgreSQL Flexible Server | `psql-mediagenerator-56mfry`, PostgreSQL 17 |
| SKU | Burstable `Standard_B1ms`, 32 GB, 7 dagers backup |
| Databaser | `mediagenerator`, `mediagenerator_test` |
| Nettverk | Offentlig tilgang, brannmurregel for én IP |

Anslått kostnad er i størrelsesorden 15 USD/måned. SKU-en kan skaleres opp uten
å opprette noe på nytt.

To ting må strammes inn før produksjon: serveren har offentlig endepunkt med en
brannmurregel per IP i stedet for private endpoint, og administratorpassordet
ligger i `.env` i stedet for i Key Vault.

**Rolletildelinger til Container App-ens managed identity:**

- `Cognitive Services OpenAI User` på AI Foundry-ressursen
- `Storage Blob Data Contributor` på lagringskontoen
- `Key Vault Secrets User` på Key Vault

Ingen hemmeligheter skal finnes i repoet eller i imaget.

---

## Docker

```bash
docker build -t mediagenerator:dev .
docker run --rm -p 8080:8080 --env-file .env mediagenerator:dev
```

Imaget bygges i flere trinn med `cargo-chef`, slik at avhengighetslaget ikke
invalideres av en kildeendring. Runtime er `debian-slim`, kjører som bruker
`app` (uid 10001) og har en `HEALTHCHECK` mot `/health`.

---

## Kvalitetsporter

Kjøres i CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) og lokalt:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

### Repository-testene

Testene i [crates/storage/tests/](crates/storage/tests/) kjører mot en ekte
PostgreSQL. De hopper over seg selv når `MEDIAGENERATOR_TEST_DATABASE_URL` ikke
er satt, slik at `cargo test` er grønn uten database — også i CI.

```bash
# Peker på en database som kan kastes. Testene migrerer den de peker på.
export MEDIAGENERATOR_TEST_DATABASE_URL="postgres://...:5432/mediagenerator_test?sslmode=require"
cargo test -p mediagenerator-storage
```

Dette er den eneste kontrollen av SQL-en, siden spørringene bruker runtime-API-et
og ikke `query!`-makroene. Kjør dem når du endrer en spørring eller skjemaet.

Regler som håndheves i kodebasen:

- `#![forbid(unsafe_code)]` i alle crates
- Ingen `unwrap()` eller `expect()` i request-håndtering
- Doc-kommentarer på engelsk på alle offentlige funksjoner og moduler
- All brukervendt tekst på norsk bokmål, samlet i `crates/domain/src/i18n.rs`

---

## Dokumenterte valg

Der spesifikasjonen ga rom for skjønn, er dette valgt:

**Database: PostgreSQL, ikke Cosmos DB.** Spesifikasjonen oppgir Cosmos DB under
teknologivalg, men prosjektstrukturen ber om et sqlx-repository, og datamodellen
er skrevet som SQL-DDL med `jsonb`, enum-typer og fremmednøkler. Det er valgt
PostgreSQL, som er det datamodellen faktisk beskriver. Cosmos DB (NoSQL API) ville
krevd at `jobs`/`assets`/`audit_log` ble denormalisert til dokumenter med
partisjonsnøkler, og Rust mangler i dag en moden offisiell Cosmos-SDK.

**`AZURE_OPENAI_API_VERSION` har ingen innebygd default.** api-version styrer
hvilke felter Azure OpenAI godtar, og en stille default ville gjort at en
feilkonfigurert deploy traff feil API uten å si fra. Verdien slås opp i
Microsoft Learn i fase 5 og dokumenteres med testet versjon i klientkoden.

**Secrets logges aldri.** `Secret`-typen i `crates/web/src/config.rs` har en
redigert `Debug`, slik at `{:?}` av hele konfigurasjonen ikke kan lekke
hemmeligheter til loggen.

**`panic = "abort"` er ikke satt i release-profilen.** Med unwinding tar en panic
i én request bare ned den requesten, ikke hele prosessen.

**HSTS sendes kun i produksjon.** Å sende HSTS over `http://localhost` ville
pinne utvikleres nettlesere til HTTPS for en host som ikke serverer det.

**Cache-Control er lagt på `/assets`-tjenesten alene**, ikke på hele routeren,
slik at API- og HTML-svar ikke arver caching.

**Korrelasjons-ID.** `x-correlation-id` gjenbrukes fra innkommende request
dersom den er kort og består av synlige ASCII-tegn uten anførselstegn; ellers
genereres en ny UUID. Det hindrer at en klient kan injisere linjeskift eller
ubegrenset data i loggstrømmen.

**CORS er tomt.** UI-et serveres fra samme origin som API-et, så ingen
cross-origin-kaller skal tillates.

**Ukjente stier gir 404, ikke omdirigering til innlogging.** Fallback-ruten er
satt utenfor `require_auth`. Ellers ville en feilskrevet URL sendt en anonym
besøkende gjennom hele innloggingsløpet for så å ende på 404.

**`SameSite=Lax`, ikke `Strict`, på sesjonscookien.** Entra ID sender brukeren
tilbake til `/auth/callback` som en top-level GET fra et annet nettsted.
Med `Strict` ville nettleseren holdt cookien tilbake på akkurat den
navigasjonen, og innloggingsløpet ville gått tapt.

**`tower-sessions` er låst til 0.14-linja.** `tower-sessions-sqlx-store` 0.15
er bygget mot `tower-sessions-core` 0.14, mens `tower-sessions` 0.15 bruker core
0.15. De to `SessionStore`-traitene er ikke samme trait, så PostgreSQL-storen
kan ikke kobles på 0.15. Oppgraderes når sqlx-storen følger etter.

**`reqwest` er pinnet til 0.12.** `openidconnect` 4.0.1 er bygget mot den, og
0.13 ville gitt to inkompatible `reqwest::Client`-typer i samme prosess.
TLS-backend er rustls; `native-tls` finnes ikke i avhengighetstreet.

**`state` sammenlignes i konstant tid.** En kortsluttende `==` ville lekket hvor
mange tegn som stemte, til en angriper som måler responstid.

**Tailwind v4 konfigureres i CSS, ikke i JS.** v4 flyttet konfigurasjonen inn i
`@theme`-blokken, så designtokenene står i
[assets/css/input.css](assets/css/input.css) og blir til CSS-variabler. De er
definert to ganger med vilje: `--color-surface` gir Tailwind-klassene
(`bg-surface`, `border-surface`), og `--surface` er navnet spesifikasjonen
bruker. Den andre er et alias til den første, så det finnes én kilde.

**`assets/css/app.css` er sjekket inn.** Alternativet var å kjøre Tailwind i
Dockerfile, som ville lagt et nedlastingssteg inn i hvert image-bygg. Nå trenger
container-imaget ingen CSS-verktøykjede.

**HTMX ligger i repoet, ikke på CDN.** CSP-en tillater `script-src 'self'` og
ingenting annet. Å åpne for en CDN bare for å slippe 50 kB i repoet ville vært
å svekke policyen for å spare plass.

**Ingen webfont lastes.** Inter brukes hvis maskinen har den, ellers systemets
UI-font. En selvhostet Inter ville lagt på ~100 kB for en forskjell få ville
lagt merke til, og CSP-en tillater uansett ikke Google Fonts.

**Brukermenyen er `<details>`/`<summary>`.** Det gir tastaturbetjening,
Escape-lukking og riktig semantikk for skjermlesere uten en linje JavaScript.

**Forslagsraden bytter i CSS, ikke JS.** En `:has()`-regel viser settet som
hører til den avkryssede radioknappen, så forslagene følger medietypen også når
JavaScript ikke kjører.

**Tegnfeltet har ikke `maxlength`.** Spesifikasjonen ber om at telleren blir rød
ved overskridelse, og det kunne den aldri blitt hvis nettleseren kuttet
innlimt tekst i stillhet. Grensen håndheves på serveren.

**Dobbel-submit-token sjekkes kun på `/api/*`.** De kallene sendes av HTMX, som
alltid legger på headeren. Utloggingsskjemaet er en vanlig nettleser-POST og
dekkes av origin-sjekken og `SameSite=Lax`. Spesifikasjonen tillater
enten-eller; her er begge på der det er praktisk mulig.

**sqlx brukes med runtime-API-et, ikke `query!`-makroene.** Makroene sjekker SQL
mot en levende database ved kompilering, som ville betydd enten en database i CI
eller en innsjekket offline-cache som må regenereres ved hver spørringsendring.
Byttet er bevisst: SQL-feil fanges av repository-testene, som kjører mot en ekte
database, og `cargo build` er uavhengig av enhver server.

**SSE-ruten ligger utenfor request-timeouten.** En hendelsesstrøm skal stå åpen
så lenge jobben varer; en 30-sekunders timeout ville kuttet den hele tiden. Alle
andre ruter har timeouten på.

**Køen er en pekepinn, ikke sannheten.** Alt som betyr noe ligger i `jobs`-tabellen.
Køen er en `mpsc`-kanal med tak på 64; når den er full feiles jobben med én gang
i stedet for å bli liggende `queued` uten at noe kommer for å hente den.

**Audit-loggen inneholder ikke prompten.** Den registrerer hvem, hva, når og
hvorfra. Prompten ligger på jobb-raden, som er underlagt oppbevaringstiden;
revisjonssporet skal ikke stille bli en ekstra kopi av innholdet.

**`x-forwarded-for` leses, men kun til audit.** Container Apps terminerer TLS
foran prosessen, så peer-adressen er ingressen. Headeren brukes aldri til en
tilgangsavgjørelse, og en forfalsket verdi kan derfor ikke gi noen noe.

**ID-tokenet lagres i sesjonen.** Det brukes som `id_token_hint` ved utlogging,
slik at Entra ID avslutter riktig sesjon uten å vise kontovelger. Det ligger
utelukkende server-side.

---

## Leveranseplan

| Fase | Innhold | Status |
| --- | --- | --- |
| 1 | Workspace, axum-server med `/health`, tracing, config, Dockerfile | Ferdig |
| 2 | Entra ID OIDC-innlogging, sesjon, `require_auth`, `/auth`-ruter | Ferdig |
| 3 | Statisk UI med Tailwind og HTMX, mock-generering | Ferdig |
| 4 | Domenemodell, migrasjoner, sqlx-repository, jobbkø, SSE | Ferdig |
| 5 | Azure-providers for bilde, lyd og video + Blob Storage og SAS | Gjenstår |
| 6 | Historikk, eksempelgalleri, rate limiting, audit-logg, tester | Gjenstår |
