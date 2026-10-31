---
layout: ../../layouts/Doc.astro
title: "Befehle"
description: "Alle Befehle und Optionen von cyberbrain 0.8.4: init, write, recall, find, propose, review, forget, scan, status, doctor, trust, install, serve, mcp, hook, policy, canary, hub und weitere."
lang: de
slug: "commands/"
---

# Befehle

```text
cyberbrain [OPTIONEN] <BEFEHL>
```

Diese Seite folgt `cyberbrain --help` und `cyberbrain <befehl> --help` von 0.8.4. Im Zweifel hat die Hilfe deines Programms recht und diese Seite nicht.

## Optionen für jeden Befehl

| Option | |
|---|---|
| `--store <STORE>` | Der Store, mit dem gearbeitet wird. Ohne Angabe wird `.cyberbrain` vom Arbeitsverzeichnis aufwärts gesucht. Auch über `CYBERBRAIN_STORE`. |
| `--json` | Maschinenlesbare Ausgabe. Jeder Befehl, der etwas ausgibt, kann das. |
| `-q`, `--quiet` | Weniger ausgeben. Fehler landen weiter auf stderr. |
| `-h`, `--help` | Hilfe. Bei manchen Befehlen ist `--help` ausführlicher als `-h`. |
| `-V`, `--version` | Version. |

## Notizen

### init

Legt einen Store und eine kommentierte `cyberbrain.toml` an.

| Option | |
|---|---|
| `--path <PFAD>` | wohin; von Haus aus `.cyberbrain` im Arbeitsverzeichnis |

### write

Schreibt eine Notiz. Ohne `--body` kommt der Text von der Standardeingabe.

```text
cyberbrain write --ring <RING> --kind <ART> --name <NAME> [--body <TEXT>] [Optionen]
```

| Option | |
|---|---|
| `--ring <RING>` | 0 bis 4; niedriger heißt vertrauenswürdiger und gewinnt einen Widerspruch. Ring 0 und 1 werden für Agenten abgelehnt. |
| `--kind <ART>` | `knowledge`, `bug`, `lesson`, `decision`, `reference`, `session` |
| `--name <NAME>` | Kurzname, eindeutig im Store |
| `--body <TEXT>` | Markdown; weglassen, um von der Standardeingabe zu lesen |
| `--tags <TAG>` | ein Tag; für mehrere die Option wiederholen |
| `--bereich <BEREICH>` | Abteilung, Team oder Fachgebiet; filtert die Suche, ändert nie die Rangfolge |
| `--retention <DAUER>` | Dauer nach ISO 8601 wie `P2Y`; ohne Angabe unbegrenzt |
| `--supersedes <NAME>` | eine Notiz, die diese ersetzt; wiederholbar. Die alte wird herabgestuft und markiert, nicht geändert. |
| `--valid-from <DATUM>` | ab wann der Inhalt gilt: `JJJJ-MM-TT` (00:00 UTC) oder RFC 3339 |
| `--invalid-at <DATUM>` | ab wann er nicht mehr gilt; die Suche findet die Notiz weiter, herabgestuft und markiert |
| `--force` | trotz PII-Fund schreiben, als `flagged` vermerkt |
| `--dry-run` | den echten Ablauf ohne Schreiben durchgehen und berichten, was passieren würde |

Findet die PII-Prüfung etwas, wird nichts geschrieben, und der Rückgabewert ist 3:

```console
$ cyberbrain write --ring 2 --kind knowledge --name contact-anna --body 'Ask anna.schmidt@example.com about the VPN.'
Write held: 1 possible personal data item(s) found.
  line 1, bytes 4..28: email [low] email shape, but a documentation domain (RFC 2606)
    anna.schmidt@example.com
Options: redact, mark reviewed, proceed flagged.
PII detection is heuristic: it finds the shapes of emails, IP addresses, API keys, IBANs and phone numbers. It misses what has no shape and flags some things that are harmless. It is a seatbelt, not a guarantee.
Nothing was written. Re-run with --force to write it flagged, or edit the body.
```

### propose

Dieselben Angaben wie bei `write` (ohne die Optionen für Gültigkeit und Ersetzung) und dieselbe PII-Prüfung, aber die Notiz landet in `proposals/`, bis jemand anderes sie annimmt. Sie wird nicht indiziert. `--force` schlägt trotz PII-Fund vor.

### review

```text
cyberbrain review [ZIEL] [--accept | --reject --reason <GRUND>] [--force] [--from-agentguard] [--dry-run]
```

Ohne Ziel: die Liste der wartenden Vorschläge. `--accept` verschiebt die Notiz in ihren Ring und indiziert sie. `--reject` braucht `--reason`, und die Vorschlagsdatei verschwindet. `--force` nimmt an, obwohl sich die Notiz seit dem Vorschlag geändert hat. `--from-agentguard` nimmt an, weil ein Administrator in AgentGuard zugestimmt hat; die beim Vorschlag festgehaltene Freigabe wird zurückgelesen und muss `approved` lauten. Wer vorgeschlagen hat, kann nicht selbst prüfen.

### invalidate

```text
cyberbrain invalidate <NAME> [--at <DATUM>] [--by <NAME>] [--clear] [--dry-run]
```

Erklärt, dass eine Notiz ab einem Tag nicht mehr gilt, ohne ihren Text umzuschreiben. Setzt `invalid_at` (und mit `--by` auch `superseded_by`), schreibt eine Audit-Zeile und indiziert neu. `--at` ist ohne Angabe jetzt; `--clear` nimmt die Erklärung zurück.

### forget

```text
cyberbrain forget <ZIEL> [--dry-run] [--report <PFAD>] [--sign]
```

Löscht eine Notiz und alles, was daraus abgeleitet ist: Datei, Abschnitte, Suchzeilen, Vektoren, Links und zwischengespeicherte Ableitungen, in einer Transaktion. Die Audit-Zeilen bleiben, sie sind der Nachweis der Löschung. `--report` schreibt einen Löschbericht, den `verify-export` prüfen kann, `--sign` signiert ihn mit dem Audit-Schlüssel des Stores. Seit 0.8.3 wird ein Agent auch hier bei Ring 0 und 1 abgewiesen. Hatte die Notiz einen Bereich und ist der Store bei einem Hub angemeldet, sagt `forget`, dass die Kopie auf dem Hub bleibt, und nennt `hub erase`.

### export

`cyberbrain export <ZIEL> [--format md|json]` gibt eine Notiz aus, mit Kopfdaten.

### import

Holt einen vorhandenen Baum von Markdown-Notizen in den Store, gesteuert über eine TOML-Zuordnungsdatei.

| Option | |
|---|---|
| `--suggest <ORDNER>` | einen Ordner ansehen und einen Plan ausgeben statt zu importieren; es wird nichts geschrieben |
| `--plan <PLAN>` | die Zuordnungsdatei: was übernommen, was übersprungen, wie geteilt, welcher Ring |
| `--accept-pii` | alle PII-Funde auf einmal annehmen |
| `--dry-run` | |

### transcribe

```text
cyberbrain transcribe <DATEI> [--ring <RING>] [--kind <ART>] [--name <NAME>] [--tags <TAG>] [--bereich <B>] [--segments] [--print] [--force] [--dry-run]
```

Schickt eine Aufnahme (WAV, MP3, M4A, OGG, WebM, FLAC oder ein Video mit Ton) an den lokalen whisper.cpp-Server aus `[transcription]` und schreibt den Text wie `write`. Von Haus aus Ring 3, Art `knowledge`, Name `transcript-<dateiname>-<datum>`, und das Tag `transcript` kommt immer dazu. `--segments` schreibt einen Absatz je Redeabschnitt mit seiner Zeit; `--print` zeigt den Text nur an und schreibt nichts. Alles außer WAV mit 16 kHz braucht einen Server, der mit `--convert` gestartet wurde.

### propose-session

Fasst eine Sitzung von Claude Code mit dem lokalen Modell zusammen und legt sie als Vorschlag ab (Ring 3, Art `session`). Braucht `[inference]`. `--transcript <PFAD>` nennt die `.jsonl`, `--latest` nimmt die neueste Mitschrift des Projekts zu diesem Store.

## Finden

### recall

```text
cyberbrain recall [ANFRAGE] [--id <ZITAT>] [-n <N>] [--ring <RING>] [--bereich <B>] [--stand <DATUM>]
```

| Option | |
|---|---|
| `--id <ZITAT>` | ein Zitat zur ganzen Notiz aufklappen statt zu suchen |
| `-n`, `--n <N>` | wie viele Treffer, von Haus aus 8 |
| `--ring <RING>` | genau dieser Ring |
| `--bereich <BEREICH>` | nur Notizen dieser Abteilung, dieses Teams oder Fachgebiets |
| `--stand <DATUM>` | Gültigkeit zu diesem Tag (`JJJJ-MM-TT`) oder Zeitpunkt (RFC 3339) beurteilen |

### find

`cyberbrain find <SYMBOL> [-l, --limit <N>]` liefert genaue Zeilenbereiche für eine Funktion, einen Typ, eine Klasse, eine Konstante oder einen Tabellennamen. Von Haus aus höchstens 20; darüber sagt das Ergebnis, dass es gekürzt ist.

### scan

Baut den Index aus dem Notizbaum neu. `--full` verwirft den Index und baut alles neu; das Audit-Log bleibt unberührt. `--dry-run`.

## Zustand

### status

Store, Index, Audit-Kette, Einbettungsprofil, der Such-Daemon, der Inferenzdienst und das Compliance-Profil mit jedem Ausgangsweg.

### doctor

Dreizehn Prüfungen in 0.8.4: waivers, notes tree, stale index, dangling links, unresolvable links, supersession, shared blocks, ring cap, session-start size, index integrity, embedding profile, audit chain, retention.

### trust

`cyberbrain trust [--grant | --revoke]` zeigt, welche Ausnahmen die `cyberbrain.toml` dieses Stores verlangt und ob du sie freigegeben hast. Siehe [Betrieb in der EU](../compliance/#ein-geklontes-repository-kann-deine-notizen-nicht-wegschicken).

## Modelle

### manifest

`cyberbrain manifest [--path <PFAD>]` schreibt die `manifest.json`, die eine Modelldatei braucht, mit dem blake3-Hash von `model.safetensors` und `tokenizer.json`. Von Haus aus `<store>/models/model2vec`.

### bench-embedding

`cyberbrain bench-embedding [--sample <N>]` misst die Einbettungsmodelle im lokalen Ollama an einer Stichprobe aus den Abschnitten dieses Stores (von Haus aus 128) und gibt den Abschnitt `[embedding]` für das passende aus. Es lädt nichts herunter.

## Agenten

### install

```text
cyberbrain install [--client claude-code|claude-desktop|codex] [--project <PFAD>] [--name <NAME>] [--undo] [--dry-run]
```

Trägt dieses Programm in die Einstellungen eines Agenten ein: die Hooks für Claude Code, den MCP-Server für einen Desktop-Client. `--client` ist wiederholbar; ohne Angabe jeder Client, der auf dem Rechner gefunden wird. `--project` ist das Projekt, dessen `.claude/settings.json` die Hooks bekommt (von Haus aus das Arbeitsverzeichnis). `--name` benennt den MCP-Eintrag (von Haus aus `cyberbrain`). Einzelheiten auf der [Seite zu Claude Code](../claude-code/#cyberbrain-install).

### hook

`cyberbrain hook <EREIGNIS>` liest die Nutzdaten des Werkzeugs von der Standardeingabe und antwortet auf der Standardausgabe. Ereignisse: `session-start`, `session-rings`, `session-context`, `user-prompt-submit`, `pre-tool-use`, `post-tool-use`, `stop`, `pre-compact`. Ein Hook lässt das Werkzeug nie scheitern: Ein interner Fehler wird festgehalten, und der Prozess endet mit 0.

### mcp

`cyberbrain mcp [--client <NAME>] [--http <ADRESSE:PORT>]`. Von Haus aus über stdio. `--client` trägt den Client als `agent:mcp:<name>` ein. `--http` bedient Streamable HTTP unter `POST /mcp` auf einer Loopback- oder privaten Adresse, mit einem Token je Client.

### mcp-client

Die Clients, die `mcp --http` aufrufen dürfen: `add <NAME> [--expires <WANN>]` gibt einmal ein Token aus und behält nur dessen Hash, `list`, `remove <NAME>`.

### guard

Fragt AgentGuard zu einem Werkzeugaufruf, ohne Store (Nutzdaten wie bei pre-tool-use auf der Standardeingabe). Für das AgentGuard-Plugin; Adresse, Schlüssel und Modus kommen aus der Umgebung. Endet immer mit 0.

## Die Weboberfläche

### serve

```text
cyberbrain serve [--port <PORT>] [--no-open] [--terminal] [--allow-resident-writes]
```

Von Haus aus Port 7777. Siehe [Weboberfläche und HTTP-API](../serve/).

## Compliance

### policy

| Unterbefehl | |
|---|---|
| `policy egress` | jeder Weg, auf dem Bytes den Rechner verlassen können, und ob er offen ist |
| `policy obligations` | was das aktive Profil über das Recht behauptet, mit Grundlage und Sicherheit |
| `policy audit` | das Audit-Log: `--limit`, `--action`, `--subject`, `--since`, `--until`, `--verify`, `--export <PFAD>`, `--signed` |
| `policy audit-key [--init]` | der Ed25519-Schlüssel des Stores für das Audit-Log und sein Fingerabdruck |
| `policy subject <KENNUNG>` | alles, was zu einer Kennung gespeichert ist (DSGVO Art. 15) |
| `policy retention [--apply] [--dry-run]` | Notizen, deren Aufbewahrungsfrist abgelaufen ist; `--apply` löscht sie über `forget` |
| `policy model-card` | Herkunft, Lizenz und Hash jeder Modelldatei im Einsatz |
| `policy attest [--yes]` | die aktuellen Dateien in Ring 0 und 1 zu denen des Betreibers erklären |
| `policy consent [--withdraw]` | die Zustimmung zum einen Modell-Download festhalten oder zurückziehen |

### verify-export

`cyberbrain verify-export <PFAD> [--expect-key <FINGERABDRUCK>]` prüft ein Audit-Bündel oder einen Löschbericht. Braucht keinen Store, keine Einstellungen und kein Netz. Endet nicht mit 0, wenn die Kette nicht hält.

### canary

Köder: `plant [--kind api-key|iban|password|token] [--label <TEXT>] [--ring <RING>]`, `list`, `remove <WELCHER>`, `push`. Siehe [Betrieb in der EU](../compliance/#köder).

## Team

### hub

Einen Hub betreiben und nutzen: `serve`, `add`, `admin reset`, `service`, `enrol`, `invite`, `push`, `pull`, `licence`, `cert`, `fleet`, `principal`, `erase`, `conflicts`, `grant`, `request`, `approve`, `requests`, `disclose`, `access-log`, `verify`, `backup`, `retention`, `alerts`, `report`, `revoke`. Die Seite [Team-Hub](../hub/) geht sie durch.

## Rückgabewerte

`0`, wenn der Befehl getan hat, was verlangt war. Eine Ablehnung durch die Regeln (ein Ring, der dem Betreiber gehört, ein PII-Fund, ein Vorschlag, den ein Agent nicht annehmen darf) endet mit `3`, der Grund steht auf stderr. Prüfungen wie `verify-export`, `hub verify`, `hub backup` und `hub service status` enden nicht mit 0, wenn das Geprüfte nicht hält. Ein geplanter Auftrag wird so zu einem fehlgeschlagenen Auftrag.

## Umgebungsvariablen

| Variable | |
|---|---|
| `CYBERBRAIN_STORE` | der Store, wie `--store` |
| `CYBERBRAIN_IDENTITY` | dein Name im Audit-Log und bei Prüfungen |
| `CYBERBRAIN_AGENT` | kennzeichnet den Aufrufer als Agenten mit diesem Namen (Claude Code setzt stattdessen `CLAUDECODE`) |
| `CYBERBRAIN_DISABLED` | die Hooks halten sich in dieser Sitzung zurück, und der Sitzungsstart sagt das |
| `CYBERBRAIN_WAIVERS` | ein anderer Ort für deine `waivers.toml` |
| `CYBERBRAIN_AGENTGUARD_KEY` | der AgentGuard-Schlüssel für `[governance]` |
| `CYBERBRAIN_HUB_TOKEN` | ein Geräte-Token für ein Dienstkonto statt der Token-Datei |
| `CYBERBRAIN_HUB_PRINCIPAL_TOKEN` | die Zugangsdaten für Hub-Befehle mit `--as` |
