---
layout: ../../layouts/Doc.astro
title: "Fünf Minuten"
description: "Store anlegen, Notiz schreiben, wiederfinden, Zitat aufklappen, eine Regel für Ring 0 vorschlagen und den Zustand prüfen. Mit der echten Ausgabe von 0.8.4."
lang: de
slug: "quickstart/"
---

# Fünf Minuten

Alles hier lief mit 0.8.4 in einem leeren Verzeichnis. Pfade sind auf `.cyberbrain/…` gekürzt. Die Ausgabe des Programms ist Englisch.

## 1. Ein Store

```console
$ cyberbrain init
Created a store at .cyberbrain
  config: .cyberbrain/cyberbrain.toml
  audit record: .cyberbrain/audit.db
  index cache: .cyberbrain/cyberbrain.db
```

Das ist der ganze Store: `notes/r0` bis `notes/r4` für das Markdown, `cyberbrain.toml` mit jeder Einstellung samt Kommentar, `audit.db` als Nachweis und `cyberbrain.db` als Zwischenspeicher, den du löschen darfst. Jeder spätere Befehl sucht den Store vom Arbeitsverzeichnis aufwärts und funktioniert deshalb überall im Projekt.

## 2. Eine Notiz

```console
$ cyberbrain write --ring 2 --kind bug --name pg18-moves-pgdata --body '# PostgreSQL 18 moves PGDATA
The official `postgres:18` image puts the data directory at `/var/lib/postgresql/18/docker` instead of `/var/lib/postgresql/data`. A volume mounted at the old path stays empty.'
wrote pg18-moves-pgdata in ring r2 as .cyberbrain/notes/r2/pg18-moves-pgdata.md (377 bytes, 1 blocks, 0 vectors, 0 links, pii: None)
id: 01M68S19BPN9WZ1YV16R34E0C0
no vectors: no model artefact at .cyberbrain/models/model2vec (expected model.safetensors and tokenizer.json); search is lexical only
```

Ohne `--body` kommt der Text von der Standardeingabe. Beim Schreiben wird der Text zuerst auf personenbezogene Daten geprüft, dann bekommt das Audit-Log eine Zeile und die Notiz wird indiziert. Einen eigenen Schritt dafür gibt es nicht; `cyberbrain scan` braucht es nur nach Änderungen von Hand.

Die letzte Zeile ist der erste ehrliche Hinweis: Es liegt noch kein Modell bereit, also gibt es keine Vektoren. Wie man eins ablegt, steht unter [Suche und Modelle](../search/).

## 3. Wiederfinden

```console
$ cyberbrain recall 'postgres data directory'
1. r2-6a203a50f6a4  r2  pg18-moves-pgdata  2026-10-31  (100% of top)
     # PostgreSQL 18 moves PGDATA
     The official `postgres:18` image puts the data directory at `/var/lib/postgresql/18/docker` instead of `/var/lib/postgresql/data`. A volume mounted at the old path stays empty.
caveat: semantic search skipped: no embedder configured; hits are lexical only
caveat: contradiction check skipped: fewer than two hits, nothing to compare
```

`r2-6a203a50f6a4` ist das Zitat: Ring 2 und zwölf Hexziffern, die den Abschnitt bezeichnen. Aufklappen:

```console
$ cyberbrain recall --id r2-6a203a50f6a4
r2-6a203a50f6a4 is block #0 of pg18-moves-pgdata (r2), ~72 tokens:

# PostgreSQL 18 moves PGDATA
The official `postgres:18` image puts the data directory at ...

--- full note ---
.cyberbrain/notes/r2/pg18-moves-pgdata.md
id: 01M68S19BPN9WZ1YV16R34E0C0  name: pg18-moves-pgdata  ring: r2  kind: bug  pii: None
blocks: r2-6a203a50f6a4
```

## 4. Eine Regel für jede Sitzung

Ring 0 und 1 kommen in jede Sitzung. Am eigenen Terminal schreibst du sie wie jeden anderen Ring. Ein Agent kann das nicht: Führt Claude Code denselben Befehl aus, wird er abgelehnt und protokolliert.

```console
$ cyberbrain write --ring 0 --kind decision --name never-push --body 'Never push without asking.'
cyberbrain: refused: refused by policy (ring-owner): ring 0 belongs to the operator; agent:claude-code:d7774d4e may not write it (note never-push). Propose the text instead — `cyberbrain propose --ring 0 --kind … --name never-push` — and the operator accepts it with `cyberbrain review never-push --accept`
```

Der Agent schlägt also vor:

```console
$ cyberbrain propose --ring 0 --kind decision --name never-push --body 'Never push without asking.'
Proposed never-push for ring 0 as .cyberbrain/proposals/never-push.md
  by: agent:claude-code:d7774d4e
  196 bytes

It is not in the index, so recall cannot find it. Somebody else runs `cyberbrain review` to see it.
```

und du entscheidest:

```console
$ cyberbrain review
1 proposal

  never-push  r0  decision
      by agent:claude-code:d7774d4e, 2026-10-31T10:45:45.141248942Z

`cyberbrain review <name> --accept`, or `--reject --reason "…"`.

$ cyberbrain review never-push --accept
```

Versucht ein Agent den letzten Befehl, wird auch das abgelehnt. Woran Cyberbrain die beiden unterscheidet und wie weit das trägt, steht unter [So funktioniert es](../concepts/#ring-0-und-1-gehören-dem-betreiber).

## 5. Ist alles in Ordnung?

```console
$ cyberbrain status
store: .cyberbrain
notes on disk: 1 (r0 0, r1 0, r2 1, r3 0, r4 0); 0 files skipped
resident rings 0+1: ~0 of 8192 tokens
index: schema v5, 1 notes, 1 blocks, 0 vectors, 0 links (0 dangling)
audit: 4 rows in .cyberbrain/audit.db, chain verified over 4 rows
embedding: no embedding model: ... search is lexical only
inference: http://127.0.0.1:11434/v1 model (none): not in use: no inference model is configured (inference.model in cyberbrain.toml); the endpoint is never contacted without one
policy: profile eu (GDPR, Regulation (EU) 2016/679), PII scan active
  ...

$ cyberbrain doctor
clean: 13 checks, nothing to report
checks: waivers, notes tree, stale index, dangling links, unresolvable links, supersession, shared blocks, ring cap, session-start size, index integrity, embedding profile, audit chain, retention
```

## Weiter

- Claude Code anbinden: `cyberbrain install --client claude-code`, siehe [Claude Code](../claude-code/).
- Alle Befehle und Optionen: [Befehle](../commands/).
