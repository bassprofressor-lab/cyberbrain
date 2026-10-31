---
layout: ../../layouts/Doc.astro
title: "Betrieb in der EU"
description: "Das Ausgangsregister, Ausnahmen, die sich ein geklontes Repository nicht selbst geben kann, die PII-Prüfung, Löschung mit signiertem Bericht, Auskunft, das verkettete und signierte Audit-Log, Audit-Exporte, Aufbewahrung, Köder und der Pflichtenkatalog."
lang: de
slug: "compliance/"
---

# Betrieb in der EU

Compliance ist hier ein Teilsystem und kein Kapitel der Dokumentation. Jede Aussage auf dieser Seite ist etwas, das ein Befehl ausgibt oder prüft. Was keiner davon tut: entscheiden, ob deine Nutzung rechtmäßig ist. Das bleibt deine Beurteilung, und das Programm sagt das an den Stellen, an denen es darauf ankommt.

`[policy] profile` ist `eu` (DSGVO, die Vorgabe), `ch` (das revidierte Schweizer DSG) oder `off`. Die Schweiz ist ein eigenes Profil und nicht „EU minus“, weil sich beide Gesetze in der Sache unterscheiden. `off` lässt die Prüfungen im Programm und schaltet sie ab.

## Das Ausgangsregister

`cyberbrain policy egress` listet jeden Weg, auf dem Bytes den Rechner verlassen können, was er trägt und ob er offen ist. In 0.8.4 sind es neun Einträge. In einem frischen Store:

| Weg | trägt Notizinhalt | Zustand in einem frischen Store |
|---|---|---|
| `ModelDownload` | nein | aus: keine `model_source` eingestellt |
| `LocalInference` | ja | erlaubt für Loopback und private Adressen; ohne `[inference] model` nie angesprochen |
| `AuditSync` | nein | aus: bei keinem Hub angemeldet |
| `NoteSync` | ja | aus: nicht angemeldet, braucht außerdem `allow_note_sync` |
| `NoteErasure` | nein | aus: nicht angemeldet |
| `HubEnrolment` | nein | auf Anforderung, einmal, beim Einlösen einer Sammeleinladung |
| `Terminal` | ja | nicht vermittelt: gibt es nur, solange `serve --terminal` läuft |
| `Governance` | ja | aus: keine `[governance] url` |
| `HubAlert` | nein | aus: kein Webhook gesetzt |

Die Liste endet mit: „That is the entire list. Telemetry does not exist.“ Welchen Zertifikaten TLS traut, entscheidet der Zertifikatspeicher des Betriebssystems; im Programm steckt keine eigene Sammlung. Die Prüfung wird je Anfrage gefragt, nicht einmal je Verbindung. Aufrufe darüber stehen im Audit-Log (die Meldungen eines Hubs in dessen eigenem Log), ebenso jede Ablehnung.

## Ein geklontes Repository kann deine Notizen nicht wegschicken

Ein Store liegt in einem Repository, seine `cyberbrain.toml` schreibt also, wer das Repository geschrieben hat. Drei Schlüssel könnten Notizen auf einen anderen Rechner schicken: `inference.allow_public_endpoint`, `inference.allow_overlay_network` und `hub.allow_public_hub`. Seit 0.8.2 zählt jeder davon nur, wenn der Store ihn verlangt **und** du ihn für diesen Store in deiner eigenen `waivers.toml` freigegeben hast (`~/.config/cyberbrain/waivers.toml`, unter Windows `%APPDATA%\cyberbrain\waivers.toml`, oder wohin `CYBERBRAIN_WAIVERS` zeigt).

```console
$ cyberbrain trust
store: .cyberbrain
waivers file: ~/.config/cyberbrain/waivers.toml
this store asks for no waivers
```

`cyberbrain trust --grant` gibt genau das frei, was der Store jetzt verlangt, `--revoke` nimmt es zurück. Beides steht im Audit-Log. Ein Schlüssel, den der Store ohne deine Freigabe setzt, ist abgeschaltet, und `doctor` sagt, welcher. Die Weboberfläche zeigt den Stand, gibt aber nichts frei und nimmt nichts zurück.

## Personenbezogene Daten beim Schreiben

Jedes `write`, `propose`, `transcribe` und `import` prüft den Text auf personenbezogene Daten, bevor etwas auf die Platte kommt. Gefunden werden die Formen von E-Mail-Adressen, IP-Adressen, API-Schlüsseln, IBANs, Telefonnummern und Passwörtern in URLs. Ein Fund hält den Schreibvorgang an, und du entscheidest: schwärzen, als geprüft markieren oder als `flagged` schreiben (`--force`). Hinter deinem Rücken wird nie geschwärzt. Die Ausgabe sagt selbst, was die Prüfung ist: „a seatbelt, not a guarantee“. Was keine erkennbare Form hat, findet sie nicht.

Über MCP darf ein Agent schwärzen oder als `flagged` schreiben, seinen eigenen Fund aber nicht als geprüft markieren; diese Wahl gehört dem Betreiber.

## Löschung (Art. 17)

```console
$ cyberbrain forget pg18-moves-pgdata --report erase.json --sign
wrote the erasure report to erase.json, signed
 pg18-moves-pgdata (01M68S19BPN9WZ1YV16R34E0C0):
  file: removed
  blocks: 1
  fts rows: 1
  vectors: 0
  outbound links: 0
  inbound links now dangling: 0
  cached derivatives: 0
  wal: emptied, nothing of the note left in the database files
  audit rows: kept (SPEC §12.2: the erasure record is the evidence)
```

Eine Transaktion entfernt die Notiz, ihre Abschnitte, Suchzeilen, Vektoren, Links und Ableitungen. Die Audit-Zeilen bleiben: Sie belegen, dass gelöscht wurde, und enthielten nie den Text der Notiz. `--dry-run` zeigt dieselbe Liste vorher. Den Bericht kann jemand anderes mit `verify-export` prüfen. Eine über einen Hub geteilte Notiz hat dort eine Kopie; `forget` sagt das und nennt `hub erase`.

## Auskunft (Art. 15)

```console
$ cyberbrain policy subject anna
# Subject access report

Identifier: `anna`
Profile: eu (GDPR, Regulation (EU) 2016/679)
Response deadline: within one month (extendable by two months)

## Notes (0 block(s))
…
## Audit log (1 row(s))
…
## Caveats

- Matching is a case-insensitive substring search over note blocks and audit rows. It finds the identifier as written; it does not find a person described without it.
```

## Das Audit-Log

`audit.db` nimmt nur Anhänge an, durchgesetzt von Triggern in der Datenbank, und jede Zeile ist mit blake3 an die vorige gekettet. `cyberbrain policy audit --verify` nennt die erste veränderte Zeile und endet nicht mit 0. Die Zeilen halten den Handelnden fest (`operator`, `agent:claude-code:<sitzung>`, `hook:pre-tool-use`, `mcp`, …), die Handlung, den Gegenstand und Einzelheiten, aber keinen Notiztext.

Eine Hashkette zeigt, dass sich eine Zeile geändert hat. Wer `audit.db` schreiben kann, könnte aber jeden Hash neu berechnen. Das schließt ein Signierschlüssel:

```console
$ cyberbrain policy audit-key --init
created audit key: 3ec4b6f7aa1148bc52e253e528263b67
public key: 09a7715f18eb4092c5e7536ec9a298dadf11dd3e4dc53bd7dbcf01b0d324c9cc
file: .cyberbrain/keys/audit.ed25519
```

Ab dann wird jede neue Zeile mit Ed25519 signiert. `serve`, `mcp` und den Hintergrunddienst neu starten, damit auch sie signieren. Seit 0.8.3 gilt ein Store mit Schlüssel, dessen Zeilen keine Signatur tragen, nicht mehr als unversehrt: `doctor` und `policy audit --verify` sagen das.

### Einen Zeitraum an jemand anderen geben

```console
$ cyberbrain policy audit --export q1.jsonl --signed
wrote 12 row(s) to q1.jsonl
checked as written: chain holds from anchor genesis
signed with key 3ec4b6f7aa1148bc52e253e528263b67
$ cyberbrain verify-export q1.jsonl --expect-key 3ec4b6f7aa1148bc52e253e528263b67
chain holds over 12 row(s)
period:   the whole log
anchor:   genesis
…
signed:   by key 3ec4b6f7aa1148bc52e253e528263b67, and the signature holds
rows:     1 signed by key 3ec4b6f7aa1148bc52e253e528263b67
rows:     11 unsigned (written before the store had an audit key; the chain covers them)

This says the file is internally intact and starts where it says it does.
Whether the anchor belongs to that machine's real history is a question only the
full log answers.
```

`--since` und `--until` (RFC 3339) schneiden einen Zeitraum heraus; seine erste Zeile verweist dann auf einen Anker, und eine Schlusszeile trägt Zeilenzahl und letzten Hash, damit an beiden Enden entfernte Zeilen auffallen. Wer die Datei bekommt, braucht nur das Programm: keinen Store, keine Einstellungen, kein Netz. Den Fingerabdruck auf einem Weg übergeben, den der Besitzer des Stores später nicht ändern kann, etwa in einem Vertrag oder einem Ticket; ohne `--expect-key` zeigt eine Signatur nur, dass irgendwer signiert hat. Ein Export ohne `--limit` umfasst den ganzen Zeitraum, weil ein still gekürzter Zeitraum der eine Fehler ist, den ein Prüfer in der Datei nicht sehen kann. Das Dateiformat und wie man es ohne dieses Programm prüft, steht in [`docs/AUDIT-EXPORT.md`](https://github.com/bassprofressor-lab/cyberbrain/blob/main/docs/AUDIT-EXPORT.md) (Englisch).

## Aufbewahrung

Eine Notiz kann eine Aufbewahrungsfrist tragen (`--retention P2Y`). `cyberbrain policy retention` listet, was abgelaufen ist; `--apply` löscht es auf demselben Weg wie `forget`. Im Hintergrund wird nichts still entfernt.

## Köder

```console
$ cyberbrain canary plant --kind api-key --label "Köder Deploy-Schlüssel"
```

Ein Köder ist ein Zugangswert, den niemand benutzt (ein API-Schlüssel, eine IBAN mit gültiger Prüfsumme ohne Bank dahinter, ein Passwort, ein Token), abgelegt in einer Notiz, die wie echt aussieht und das Tag `canary` trägt. Wird er irgendwo außerhalb seiner eigenen Notiz benutzt, wird das abgelehnt: in jedem Werkzeugaufruf, den der Hook vor dem Aufruf sieht, und in jeder geschriebenen oder vorgeschlagenen Notiz. Jede Benutzung wird als `canary.hit` festgehalten, und ein Hub, der diese Zeile bekommt, meldet sie. Der Wert selbst liegt in `keys/canaries.json`, nur für den Besitzer lesbar, und taucht nie im Log, in einer Ablehnung oder einer Meldung auf; dort steht sein Fingerabdruck, zwölf Hexziffern seines SHA-256. `canary list` zeigt die Köder und ihre Treffer, `canary push` gibt sie an AgentGuard.

## Was das Profil behauptet

```console
$ cyberbrain policy obligations
Profile eu: GDPR, Regulation (EU) 2016/679
11 obligation(s) this profile encodes.

Scope  [high]
  Applies to personal data of natural persons. A memory store is in scope as soon as a note names, describes or identifies a person.
  basis: GDPR Art. 2, Art. 4(1)
  note: Whether a given store contains personal data at all is a fact about its content, not something this tool can assert.
…
```

Jede Zeile trägt ihren Artikel und eine Sicherheit. Was nicht mit hoher Sicherheit feststeht, steuert kein Verhalten.

## Modelle

`cyberbrain policy model-card` nennt Herkunft, Lizenz und Hash jeder Modelldatei im Einsatz. Ein Modell-Download ist nur zum genauen Host in `[embedding] model_source` möglich, nur nach `cyberbrain policy consent`, und der Hash der Datei wird danach geprüft. Ohne eingestellte Quelle, und das ist die Vorgabe, lässt sich überhaupt nichts herunterladen.
