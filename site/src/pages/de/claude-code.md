---
layout: ../../layouts/Doc.astro
title: "Claude Code: Hooks, MCP, Schutz"
description: "Cyberbrain an Claude Code anbinden: cyberbrain install, die sechs Hook-Ereignisse, der Store-Schutz vor jedem Werkzeugaufruf, AgentGuard im Modus shadow oder enforce und der MCP-Server für Claude Desktop und andere."
lang: de
slug: "claude-code/"
---

# Claude Code: Hooks, MCP, Schutz

Cyberbrain erreicht einen Agenten auf zwei Wegen. Die **Hooks** laufen an festen Punkten einer Sitzung von Claude Code: Sie geben Ring 0 und 1 in die Sitzung und prüfen jeden Werkzeugaufruf, bevor er läuft. Der **MCP-Server** lässt einen Agenten während der Arbeit suchen und schreiben, und er ist der Zugang für Clients ohne Hooks, etwa Claude Desktop.

## cyberbrain install

```console
$ cyberbrain install --client claude-code
Setting up /usr/local/bin/cyberbrain
  store: /home/du/projekt/.cyberbrain

claude-code
  added           /home/du/projekt/.claude/settings.json
                  the hooks are per project, so this is the project you named

Restart the client for it to read this. `cyberbrain install --undo` takes it back out.
```

Es schreibt je Ereignis einen Eintrag in die `.claude/settings.json` des Projekts. Jeder Eintrag trägt `"_managedBy": "cyberbrain"`, nennt den absoluten Pfad des Programms, das `install` ausgeführt hat, und hat ein Zeitlimit von 30 Sekunden:

| Ereignis | Befehl | Matcher |
|---|---|---|
| `SessionStart` | `hook session-rings`, dann `hook session-context` | alle |
| `UserPromptSubmit` | `hook user-prompt-submit` | alle |
| `PreToolUse` | `hook pre-tool-use` | `Edit\|Write\|MultiEdit\|NotebookEdit\|Bash\|WebFetch` |
| `PostToolUse` | `hook post-tool-use` | `Edit\|Write\|MultiEdit\|NotebookEdit` |
| `Stop` | `hook stop` | alle |
| `PreCompact` | `hook pre-compact` | alle |

Was in der Datei nicht von uns stammt, bleibt unverändert, und die vorige Fassung bleibt daneben liegen. Seit 0.8.3 wird die Datei als Ganzes ersetzt, ein abgebrochener Schreibvorgang hinterlässt also keine halbe Einstellungsdatei; ist sie ein Link, bleibt sie einer. Wer das Programm verschiebt oder neu installiert, führt `install` noch einmal aus. `--dry-run` sagt, was sich ändern würde.

Ohne `--client` richtet `install` jeden Client ein, den es auf dem Rechner findet: Claude Code für das Projekt, einen MCP-Eintrag für Claude Desktop. Bei Codex CLI ändert es die von Hand geschriebene `config.toml` nicht, sondern gibt die Zeilen zum Einfügen aus.

## Die Hooks

Ein Hook lässt das Werkzeug nie scheitern. Jeder interne Fehler wird festgehalten, und der Prozess endet mit 0 und leerer Ausgabe. Die Spezifikation verlangt im 99. Perzentil 15 ms für jedes Ereignis außer dem Sitzungsstart (150 ms); das README von 0.8.4 nennt 7 bis 9 ms und 17 ms für den Sitzungsstart, an einem Store mit 1.300 Notizen und mit Prozessstart.

**Beim Sitzungsstart** kommen Ring 0 und 1, jeder Abschnitt mit seinem Zitat, danach die neueste Übergabenotiz, eine kurze Übersicht über den Store und wie man ihn benutzt. Claude Code zeigt die Ausgabe eines Hooks nur bis 10.000 Zeichen vollständig, deshalb ist das seit 0.7.10 auf zwei Hooks verteilt: zuerst und allein `session-rings`, danach `session-context`. Installationen von vor 0.7.10 rufen `session-start` auf, das beides auf einmal tut.

Die **Übergabe** ist die neueste Notiz in Ring 2 oder 3, deren Name `handoff` enthält (einstellbar unter `[handoff]`), höchstens sieben Tage alt. Sie wird als das gezeigt, was sie ist: datiert und nachrangig zu Ring 0 und 1. Eine Sitzung findet so, was die vorige ihr hinterlassen hat, ohne danach fragen zu müssen.

**Bei jeder Eingabe** (`user-prompt-submit`) vergleicht der Hook Ring 0 und 1 mit dem, was eingespeist wurde. Hat der Betreiber sie während der Sitzung geändert, kommt der neue Text mit der nächsten Eingabe. Gab es nie einen Sitzungsstart, werden sie jetzt eingespeist.

**Nach einem Werkzeugaufruf** (`post-tool-use`) bemerkt der Hook eine direkte Änderung an einer Store-Datei und sagt dem Modell, was daraus folgt: Der Index antwortet bis zum nächsten `cyberbrain scan` mit dem alten Text, eine kaputte `cyberbrain.toml` hält jeden Befehl an.

**Stop** und **pre-compact** zählen Runden und Verdichtungen. Pre-compact kann nichts einspeisen, weil Claude Code für dieses Ereignis keinen Weg zum Modell vorsieht; Wissen trägt der Sitzungsstart, der auf die Verdichtung folgt.

Mit `CYBERBRAIN_DISABLED` halten sich die Hooks in einer Sitzung zurück, und der Sitzungsstart sagt das, damit Stille nie mit einem kaputten Hook verwechselt wird.

## Der Store-Schutz

Vor jedem Aufruf von `Edit`, `Write`, `MultiEdit`, `NotebookEdit` und `Bash` prüft der Hook, ob er den Store berührt. Das sind echte Antworten von 0.8.4:

| Aufruf | Antwort |
|---|---|
| `Edit .cyberbrain/notes/r0/a.md` | **deny**: eine Notiz in Ring 0, Regeln des Betreibers |
| `Write .cyberbrain/notes/r2/a.md` | **ask**: eine direkte Änderung umgeht PII-Prüfung, Audit-Zeile und Neuindizierung; besser `cyberbrain write` |
| `echo x >> .cyberbrain/cyberbrain.toml` | **ask**: die Einstellungen enthalten Zustimmung, Ausgangswege und Governance, und eine Zeile, die sich nicht lesen lässt, hält jeden Befehl an |
| `rm .cyberbrain/keys/audit.ed25519` | **deny**: der Signierschlüssel ist Nachweis |
| `rm .cyberbrain/audit.db` | **deny**: das Audit-Log ist ein Nachweis, an den nur angehängt wird |
| `cat .cyberbrain/notes/r2/….md` | erlaubt: Lesen ist in Ordnung |
| ein Befehl, der `CLAUDECODE` um einen `cyberbrain`-Aufruf herum entfernt | **deny**: er würde als Betreiber eingetragen |

Jede Ablehnung ist eine Audit-Zeile. Seit 0.8.3 fängt der Schutz auch die üblichen Umwege: Schreibweisen wie `notes/./r0` oder Platzhalter wie `n*/r0`, ein `cd` in den Store vorweg, `git checkout|restore|rm|mv`, `find -delete`, `xargs rm`, eingebetteter Code für python, perl, node, ruby und php, Skripte über `sh -c` und Links auf den Store. Unter macOS ignoriert er Groß- und Kleinschreibung, wie das Dateisystem.

Lässt sich ein Store nicht öffnen, etwa weil `cyberbrain.toml` nicht lesbar ist, wird jeder Aufruf, der etwas ändert, einem Menschen vorgelegt, Lesen geht weiter. Vor 0.8.3 bekam der Hook in diesem Fall keine Antwort, und Claude Code liest das als „erlaubt“.

Es bleibt eine Heuristik über den Befehlstext, keine Sandbox.

### Köder

Hat der Store [Köder ausgelegt](../compliance/#köder), lehnt derselbe Hook jeden Werkzeugaufruf ab, der einen davon außerhalb seiner eigenen Notiz benutzt, noch bevor AgentGuard gefragt wird, und trägt ihn als `canary.hit` ein.

## AgentGuard: shadow und enforce

Mit einem Abschnitt `[governance]` schickt der Hook außerdem jeden Aufruf von Bash, Edit, Write, MultiEdit, NotebookEdit und WebFetch an einen AgentGuard-Dienst und setzt dessen Antwort um (allow, deny, ask). Der eigene Schutz des Stores entscheidet zuerst.

```toml
[governance]
url = "http://127.0.0.1:<port>"  # der AgentGuard-Dienst; nur Loopback oder privates Netz
tenant = "…"
agent_id = "claude-code"
mode = "shadow"                 # oder "enforce"
timeout_ms = 1000
```

Der Schlüssel kommt aus `CYBERBRAIN_AGENTGUARD_KEY` oder `~/.config/cyberbrain/agentguard.key`, nie aus dem Store.

- `shadow` hält die Antwort fest und hält nichts an, auch nicht, wenn der Dienst nicht erreichbar ist. Die Log-Zeile sagt, was AgentGuard entschieden hätte.
- `enforce` setzt deny und ask um. Ist der Dienst nicht erreichbar, gehen reine Lesezugriffe durch, und zu allem anderen wird gefragt.

Seit 0.8.2 schreibt `cyberbrain install` in einem Projekt mit `enforce` an den Hook `pre-tool-use` zusätzlich `"onFailure": "block"` (Claude Code 2.1.295 und neuer): Ein Hook, der nicht startet, zu lange braucht oder Unlesbares ausgibt, blockiert dann den Aufruf, statt ihn durchzulassen. In einem bestehenden enforce-Projekt `install` noch einmal ausführen, um das Feld zu bekommen. Im Modus shadow und bei den anderen Hooks bleibt es beim Standard, damit ein fehlendes Programm dort nicht jede Änderung anhält.

## Der MCP-Server

`cyberbrain mcp` spricht das Model Context Protocol über stdio und bietet fünf Werkzeuge:

| Werkzeug | Argumente | |
|---|---|---|
| `recall` | `query`, `n`, `ring`, `bereich`, `stand` | hybride Suche mit Zitaten und Hinweisen |
| `recall_id` | `citation` | der zitierte Abschnitt und seine ganze Notiz |
| `find` | `symbol`, `limit` | Zeilenbereiche aus dem Code-Index |
| `write` | `ring`, `kind`, `name`, `body`, `tags`, `bereich`, `retention`, `supersedes`, `valid_from`, `invalid_at`, `force`, `choice`, `expected_updated`, `dry_run` | eine Notiz schreiben; Antwort `written`, `held` oder `conflict` |
| `status` | | derselbe Bericht wie `cyberbrain status --json` |

Ein zurückgehaltener Schreibvorgang (PII gefunden) oder eine Ablehnung kommt als Werkzeugergebnis mit `isError` und den Funden zurück, damit der Agent darauf reagieren kann. Die Hinweise stehen in `structuredContent.caveats` und im Text. Das Werkzeug `write` kommt an Ring 0 und 1 nicht heran.

Für Claude Code in einem Projekt:

```console
$ claude mcp add cyberbrain -- cyberbrain mcp
```

In einem Projekt mit installierten Hooks ist das freiwillig: Die Hooks geben dem Agenten die Ringe schon, und `cyberbrain recall` kann er über Bash aufrufen.

### Claude Desktop und andere Clients ohne Hooks

`cyberbrain install --client claude-desktop` schreibt in jede gefundene Installation von Claude Desktop einen MCP-Eintrag, der `cyberbrain mcp --store <pfad>` startet. Ein Desktop-Client hat kein Projekt, ein zweiter Store auf demselben Rechner braucht deshalb einen zweiten Eintrag: `--name`.

Ein solcher Eintrag hat kein `--client`, im Audit-Log steht er als `mcp`. Das ist Absicht: **Die Suche über MCP ohne Client-Namen liefert Ring 0 und 1 wie jeden anderen Ring.** Ein Agent in Claude Code bekommt diese Ringe beim Sitzungsstart, und die Suche lässt sie weg, damit er sie nicht doppelt hat. Claude Desktop hat keinen Hook beim Sitzungsstart; ohne die Ringe in der Suche sähe es die Regeln des Betreibers nie. Wer den Client benennt (`--client <name>`), bekommt ihn als `agent:mcp:<name>` eingetragen und kann ihn über `[provenance] untrusted_clients` und `[clients.bereiche]` einschränken.

### MCP über HTTP

Für einen Client, der auf diesem Rechner keinen Prozess starten kann, etwa n8n in einem Container:

```console
$ cyberbrain mcp-client add n8n --expires P90D
$ cyberbrain mcp --http 172.17.0.1:7790
```

`mcp-client add` gibt das Token einmal aus und behält nur seinen Hash, im eigenen Konfigurationsverzeichnis, nie im Store. Jede Anfrage braucht `Authorization: Bearer <token>` und läuft als `agent:mcp:<name>`. Die Adresse muss Loopback oder privat sein, `0.0.0.0` wird abgelehnt, TLS gibt es nicht, und eine Anfrage mit `Origin`-Kopfzeile wird abgelehnt, Browser kommen also nicht heran. Ein abgelaufenes Token wird mit 401 und dem Namen des Clients abgewiesen, und die Ablehnung ist eine Audit-Zeile.
