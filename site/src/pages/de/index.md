---
layout: ../../layouts/Doc.astro
title: "Cyberbrain – Dokumentation"
description: "Zitiertes, vertrauensgestuftes Gedächtnis für KI-Coding-Agenten, lokal zuerst. Installation, Ringe und Zitate, Befehle, Hooks und MCP für Claude Code, Bedeutungssuche, Team-Hub und die Compliance-Schicht für die EU."
lang: de
slug: ""
---

# Cyberbrain

<p class="lead">Zitiertes, vertrauensgestuftes Gedächtnis für KI-Coding-Agenten, lokal zuerst. Ein natives Programm, Notizen als einfaches Markdown, und jeder Weg, auf dem Daten den Rechner verlassen könnten, steht an einer Stelle, die du ausdrucken kannst.</p>

Ein Agent vergisst zwischen zwei Sitzungen alles. Cyberbrain gibt ihm Notizen, die bleiben: Markdown-Dateien im eigenen Repository, eine hybride Suche darüber und an jedem Treffer ein Zitat, das auf genau den Abschnitt zurückführt, aus dem er stammt. In den Ringen 0 und 1 stehen die Regeln des Projekts, sie kommen in jede Sitzung. Der Agent darf Änderungen daran vorschlagen, annehmen kann sie nur ein Mensch.

<div class="btns">
  <a class="btn primary" href="install/">Installation</a>
  <a class="btn ghost" href="quickstart/">Fünf Minuten</a>
  <a class="btn ghost" href="https://github.com/bassprofressor-lab/cyberbrain">Quelltext auf GitHub</a>
</div>

```console
$ cyberbrain recall 'postgres data directory'
1. r2-6a203a50f6a4  r2  pg18-moves-pgdata  2026-10-31  (100% of top)
     # PostgreSQL 18 moves PGDATA
     The official `postgres:18` image puts the data directory at `/var/lib/postgresql/18/docker` ...
caveat: semantic search skipped: no embedder configured; hits are lexical only
caveat: contradiction check skipped: fewer than two hits, nothing to compare
```

Die Hinweise am Ende zählen so viel wie der Treffer. Cyberbrain sagt, was es **nicht** getan hat: Hier lag kein Modell bereit, also wurde nur nach Wörtern gesucht, und bei einem einzigen Treffer gab es nichts zu vergleichen. Ein ungeprüftes Ergebnis sieht nie aus wie ein geprüftes.

<div class="cards">
  <div class="card"><div class="k">Version</div><div class="v">0.8.4</div><div class="s">erschienen am 31.10.2026</div></div>
  <div class="card"><div class="k">Programm für Linux</div><div class="v">20 MB</div><div class="s">mit Weboberfläche</div></div>
  <div class="card"><div class="k">Plattformen</div><div class="v">3</div><div class="s">Linux, Windows, macOS (Apple-Chip)</div></div>
  <div class="card"><div class="k">Telemetrie</div><div class="v">keine</div><div class="s">es gibt keinen Code dafür</div></div>
</div>

<sub>Größe der Release-Datei `cyberbrain-linux-x86_64` von 0.8.4 (20.024.408 Bytes).</sub>

## Vier Zugänge

Alle vier sind dasselbe Programm und arbeiten auf demselben Store.

| | |
|---|---|
| [`cyberbrain <befehl>`](commands/) | alles über die Kommandozeile, `--json` bei jedem Befehl, der etwas ausgibt |
| [`cyberbrain hook <ereignis>`](claude-code/#die-hooks) | Ereignisse von Claude Code: Ring 0 und 1 beim Sitzungsstart, der Store-Schutz vor jedem Werkzeugaufruf |
| [`cyberbrain mcp`](claude-code/#der-mcp-server) | Model Context Protocol über stdio (oder HTTP mit einem Token je Client): `recall`, `recall_id`, `find`, `write`, `status` |
| [`cyberbrain serve`](serve/) | Weboberfläche und HTTP-API, nur auf Loopback, nur für das eigene Konto |

## Wie es weitergeht

- Neu hier: [Installation](install/), danach [fünf Minuten](quickstart/).
- Was Ringe, Zitate und Vorschläge sind: [So funktioniert es](concepts/).
- Anbindung an einen Agenten: [Claude Code](claude-code/).
- Bessere Suche als Stichwörter: [Suche und Modelle](search/).
- Mehr als ein Rechner: [Team-Hub](hub/).
- Personenbezogene Daten, Prüfungen, Löschung: [Betrieb in der EU](compliance/).
- Nur das kleine Programm: [Light oder voll](light-or-full/).

## Stand

0.8.4 ist jung. Das README dieser Version zählt 1.097 Tests und 18 Browsertests und sagt, dass es an einem echten Bestand eines Betreibers lief (1.086 Notizen in fünf Projekten) und an wenig sonst. Das Windows-Installationsprogramm ist der neueste Teil. Wenn etwas hakt, hilft ein [Issue](https://github.com/bassprofressor-lab/cyberbrain/issues).

Lizenz: [FSL-1.1-ALv2](https://github.com/bassprofressor-lab/cyberbrain/blob/main/LICENSE.md). Für alles nutzbar außer für ein Konkurrenzprodukt; jede Version wird zwei Jahre nach ihrem Erscheinen Apache-2.0.
