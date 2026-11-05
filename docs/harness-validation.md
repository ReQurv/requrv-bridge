# Validazione harness: macOS e Windows

La configurazione corretta non basta a certificare una sessione funzionante. Questa procedura separa i test dei file dal collaudo dell'app installata e del gateway. Windows va collaudato su una macchina Windows nativa: WSL ha home, processi e installazioni differenti.

## Contratto comune

Ogni superficie deve essere rilevata separatamente, ricevere modello e chiave Hive, conservare le impostazioni estranee a Bridge e permettere il ripristino. La configurazione è persistente: resta dopo la chiusura di Bridge e vale anche per gli avvii manuali dell'harness. Le superfici che condividono i file condividono anche configurazione e ripristino.

Il flusso è: rilevamento → controllo gateway → backup e configurazione → avvio, oppure richiesta di riavvio. Il riavvio deve caricare lo stato già scritto, incluso quello appena ripristinato. Per Claude Desktop il ripristino viene riapplicato dopo la chiusura perché l'app può salvare i propri file durante l'uscita.

## Matrice delle superfici

| Superficie | Protocollo | Configurazione condivisa | macOS | Windows |
| --- | --- | --- | --- | --- |
| OpenCode app | Chat Completions | CLI OpenCode | Da collaudare | Da collaudare |
| OpenCode CLI | Chat Completions | App OpenCode | Da collaudare | Da collaudare |
| ChatGPT app, modalità Codex | Responses HTTP/SSE | Codex CLI | Da collaudare | Da collaudare |
| Codex CLI | Responses HTTP/SSE | ChatGPT, modalità Codex | Da collaudare | Da collaudare |
| Claude Code CLI | Anthropic Messages | Nessuna app desktop | Da collaudare | Da collaudare |
| Claude Desktop, chat 3p | Anthropic Messages | Nessuna CLI | Da collaudare | Da collaudare |
| Hermes app | Chat Completions | CLI, home e profili Hermes | Da collaudare | Da collaudare |
| Hermes CLI | Chat Completions | App, home e profili Hermes | Da collaudare | Da collaudare |

Le celle si riferiscono al collaudo completo di questa revisione. Test unitari e build non cambiano automaticamente questi esiti. Il supporto Linux preesistente viene mantenuto e coperto dalla CI; questa campagna riguarda macOS e Windows.

### Prova minima eseguita il 23 ottobre 2026

Sul Mac di sviluppo, usando il modello `requrv-small-3.8`, configurazioni temporanee e una richiesta senza strumenti:

| CLI | Versione installata | Generazione minima |
| --- | --- | --- |
| Codex | 0.163.0 | Risposta attesa ricevuta, exit 0 |
| Claude Code | 2.1.301 | Risposta attesa ricevuta, exit 0 |
| OpenCode | 2.1.2 | Risposta attesa ricevuta, exit 0 |
| Hermes | 0.22.2 | Non conclusiva: l'avvio nell'ambiente isolato ha avviato `uv sync --frozen`; runtime e sincronizzazione sono stati interrotti prima della generazione |

Questa prova conferma una generazione attraverso i primi tre client e i protocolli configurati. Non collauda l'intero flusso dell'interfaccia Bridge, la console interattiva, il cambio chiave/modello, gli strumenti o il ciclo di vita desktop. Il risultato Hermes riguarda il bootstrap nell'ambiente isolato e non dimostra un errore del gateway. Non sono state avviate finestre desktop o eseguiti test reali su Windows.

## File gestiti

`~` indica la home dell'utente; su Windows normalmente `%USERPROFILE%`.

| Harness | File | Backup |
| --- | --- | --- |
| OpenCode | `~/.config/opencode/opencode.jsonc`, oppure il `.json` già esistente | Backup adiacente `.bak` |
| Codex / ChatGPT | `~/.codex/config.toml`, `auth.json`, `hive-models.json` | Backup adiacenti `.hive.bak` |
| Claude Code | `~/.claude/settings.json`; approvazione chiave in `~/.claude.json` | `settings.json.hive.bak`; l'approvazione locale è distinta dal routing |
| Claude Desktop macOS | `~/Library/Application Support/Claude/` e `Claude-3p/` | Backup adiacenti `.hive.bak` |
| Claude Desktop Windows | `%LOCALAPPDATA%/Claude/` e `Claude-3p/` | Backup adiacenti `.hive.bak` |
| Hermes macOS | `$HERMES_HOME`, altrimenti `~/.hermes`, file `config.yaml` e `profiles/*/config.yaml` | Backup adiacenti `.hive.bak` |
| Hermes Windows | `%HERMES_HOME%`, altrimenti `%LOCALAPPDATA%/hermes`, file e profili come sopra | Backup adiacenti `.hive.bak` |

Il merge YAML di Hermes conserva gli altri valori, ma può normalizzare formattazione e commenti. Il backup conserva i byte originali.

Il backup rappresenta lo stato prima del primo passaggio a Hive. Il ripristino del file intero può quindi annullare anche modifiche aggiunte in seguito: controllare il backup prima di usarlo su una configurazione importante. La cancellazione della chiave da Bridge riguarda il suo archivio locale, non revoca la chiave e non equivale al ripristino degli harness.

MCP è una funzione separata: al momento i target sono Claude Code, OpenCode e Claude Desktop. Non viene promessa la distribuzione MCP a Codex o Hermes.

## Collaudo ripetibile su entrambe le piattaforme

Usare un account di prova o una copia delle configurazioni. Registrare versione di Bridge, sistema operativo, architettura, versione di ogni harness e tipo di installazione. Per Windows provare almeno Microsoft Store per ChatGPT/Claude, installer OpenCode e CLI native; annotare esplicitamente eventuali installazioni da npm o directory personalizzate.

Per ciascuna delle otto righe:

1. Aprire Bridge dal sistema operativo, senza un terminale che gli fornisca un PATH arricchito. Premere «Rileva». Controllare che app e CLI siano distinte e che un installer non conti come app funzionante.
2. Salvare una chiave valida e selezionare un modello testuale. Per le CLI scegliere una cartella di prova il cui percorso contenga spazi, apostrofo e caratteri Unicode, ad esempio `Progetto d'esempio à`.
3. Avviare con l'app chiusa. Verificare cartella di lavoro, modello effettivo e connessione Hive. Nelle CLI devono funzionare tastiera, rendering e Ctrl+C.
4. Inviare un messaggio breve e verificare che lo streaming termini senza errore. In una seconda richiesta chiedere di leggere un file della cartella di prova, poi modificarne uno e verificare il contenuto sul disco. Per la chat Claude Desktop verificare le funzioni effettivamente disponibili nella superficie chat; il Code tab non è il target di questa integrazione.
5. Chiudere Bridge e aprire manualmente l'harness. Verificare che utilizzi ancora Hive. Se una CLI è stata configurata da Bridge, controllare anche l'app che condivide i suoi file e viceversa.
6. Cambiare modello e rilanciare. Con l'app già aperta, provare sia «Annulla» sia la conferma del riavvio: annullare deve lasciare la modifica pronta per l'avvio successivo, confermare deve caricarla subito.
7. Con tutti i target già configurati, cambiare chiave. Controllare il funzionamento di ogni target, inclusi i profili Hermes. Nessuna chiave va copiata nel rapporto di prova.
8. Ripristinare mentre l'app è aperta e confermare il riavvio. Verificare che Hive non venga riscritto, che lo stato originale ritorni e che il pulsante di ripristino si aggiorni. Ripetere il ciclo configura → ripristina per escludere backup dello stato Hive al posto dell'originale.
9. Provare chiave non valida, gateway irraggiungibile, endpoint assente e configurazione malformata in un ambiente di prova. Il launcher deve segnalare l'errore e preservare i file non validi.

Per Hermes ripetere con profilo predefinito e profilo nominato, mantenendo un altro provider e opzioni del modello nel file. Per Codex ripetere il ripristino dopo aver effettuato un nuovo login OAuth: una sessione più recente non deve essere sovrascritta da un vecchio backup.

Il risultato minimo da registrare è:

```text
Revisione Bridge:
OS / architettura:
Harness / versione / installazione:
Rilevamento:
Configurazione e avvio a freddo:
Modello effettivo osservato sul gateway:
Streaming / strumenti:
Avvio manuale dopo uscita da Bridge:
Cambio modello / chiave:
Annullamento e conferma riavvio:
Ripristino a caldo e secondo ciclo:
Esito / errore riproducibile:
```

## Cosa provano i controlli automatici

Da una checkout con i prerequisiti Tauri installati:

```sh
bun install --frozen-lockfile
bun run lint
bun run typecheck
cd src-tauri
cargo test
cargo clippy -- -D warnings
cd ..
bun run tauri build --no-bundle
```

La CI esegue test Rust e Clippy su macOS, Windows e Linux. I test di configurazione usano directory temporanee; i PoC ignorati che operano sulla home reale non fanno parte della suite normale. I test dei builder Windows possono girare su macOS, ma non dimostrano il comportamento della console Windows. Il binario locale macOS è `src-tauri/target/release/app`; su Windows è `src-tauri/target/release/app.exe`.

Un probe con corpo vuoto verifica disponibilità e autenticazione della rotta, non la compatibilità del protocollo completo: non prova scelta del modello, tool calling, streaming, immagini o ripresa di una sessione.

## Limiti da verificare sul gateway

- Claude Desktop pubblica lo slot Anthropic `claude-sonnet-5`, etichettato con il modello scelto. Il nome visualizzato non modifica il modello nel payload. Il gateway deve risolvere quello slot al modello previsto; la verifica deve osservare l'instradamento effettivo. Per una scelta arbitraria indipendente dagli alias server servirebbe una funzione di mapping del gateway o un proxy locale dedicato.
- ChatGPT viene configurato nella superficie Codex. La normale chat ChatGPT e la voce non sono instradate da questa configurazione.
- OpenCode può avere configurazioni di progetto e variabili d'ambiente con precedenza sulla configurazione globale. Il collaudo va ripetuto sia in una cartella vuota sia nel progetto reale.
- Le installazioni Windows Store e gli installer classici hanno percorsi diversi. La compilazione di un ramo Windows non dimostra il rilevamento di ogni variante di installer.

## Confronto con altri launcher

Fonti consultate il 23 ottobre 2026:

- [Ollama Launch: registro degli adapter](https://github.com/ollama/ollama/blob/main/cmd/launch/registry.go) separa le integrazioni e le relative regole di installazione. È un riferimento utile per separare progressivamente il modulo `commands.rs` per responsabilità.
- [Ollama: integrazione Claude Desktop](https://github.com/ollama/ollama/blob/main/cmd/launch/claude_desktop.go) distingue applicazione del profilo, ciclo di vita e gateway locale. Bridge mantiene invece un collegamento diretto a Hive: aggiungere un proxy cambierebbe la necessità di tenere Bridge aperto.
- [CC Switch: gestione dei file](https://github.com/farion1231/cc-switch/blob/main/docs/user-manual/en/5-faq/5.1-config-files.md) documenta backup iniziali, impostazioni globali separate dai campi provider e stato locale di ciò che il programma ha scritto. Per Bridge sono particolarmente utili la conservazione dei provider estranei e la protezione del login al ripristino.
- [Hermes: resolver della home](https://github.com/NousResearch/hermes-agent/blob/main/hermes_constants.py) definisce il percorso nativo Windows, diverso da quello POSIX.
- [OpenAI Docs: configurazione](https://learn.chatgpt.com/docs/config-file/config-reference) descrive provider personalizzati e configurazione Codex; le caratteristiche effettive vanno confermate con la versione installata.
- [Microsoft: creazione della console](https://learn.microsoft.com/en-us/windows/console/creation-of-a-console) descrive perché una CLI interattiva richiede una console e handle adeguati.

Non vengono aggiunti nuovi harness in questa revisione. La priorità è rendere verificabile e coerente il contratto degli otto target esistenti.
