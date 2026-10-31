---
layout: ../../layouts/Doc.astro
title: "Suche und Modelle"
description: "Stichwortsuche von Anfang an, Bedeutungssuche mit einem statischen Einbettungsmodell, das du selbst ablegst, ein Transformer über ein lokales Ollama, der optionale Inferenzdienst für Widersprüche und Sprache zu Text auf dem eigenen Rechner."
lang: de
slug: "search/"
---

# Suche und Modelle

Von Anfang an sucht Cyberbrain nach Wörtern, und jedes Ergebnis sagt das in einem Hinweis. Nach Wörtern heißt wörtlich: Ohne Modell findet `postgres` nicht `PostgreSQL`. Die Bedeutungssuche braucht ein Modell, und **heruntergeladen wird nie etwas in deinem Namen**. Jedes Modell auf dieser Seite legst du selbst ab.

## Statische Einbettungen: ein Laptop reicht

Das Standardverfahren heißt `model2vec`: statische Einbettungen, also ein Nachschlagen in einer Tabelle und ein Mittelwert, kein Durchlauf durch ein neuronales Netz. Ein Prozessorkern genügt. Die Datei auf der Platte ist größer als die Arbeit, die sie macht, denn den Platz braucht der Wortschatz.

Drei Schritte, alle von dir:

```console
$ # 1. model.safetensors und tokenizer.json unter .cyberbrain/models/model2vec ablegen,
$ #    etwa minishlab/potion-multilingual-128M (~500 MB) oder potion-base-8M (~30 MB), beide MIT
$ cyberbrain manifest   # 2. hält den blake3-Hash jeder Datei fest
$ cyberbrain scan       # 3. Vektoren entstehen beim Indizieren einer Notiz
```

Darauf kommt es beim Manifest an: Eine Modelldatei, die nicht zu den festgehaltenen Hashes passt, wird abgelehnt statt geladen. Ein unter dem Store ausgetauschtes Modell ändert, was seine Vektoren bedeuten, ohne dass man es sieht. Gehören die gespeicherten Vektoren zu einem anderen Profil, schaltet sich die Bedeutungssuche ab und sagt das deutlich, statt Vektoren zu vermischen.

Was es kostet, laut README von 0.8.4:

| | ohne Modell | mit Einbettungsmodell |
|---|---|---|
| eine Suche | 10 ms | 2,3 s, vor allem für das Laden des Modells |
| Speicher | 14 MB | 1,55 GB in der Spitze während der Suche |
| Index komplett neu | 0,7 s | 4,0 s |
| auf der Platte | — | 507 MB für die Modelldatei |

<sub>Gemessen am 06.09.2026 an einem Store mit 1.004 Notizen und 4.031 Abschnitten auf einem Server ohne Grafikkarte (AMD EPYC-Milan, 12 vCPU, 23 GB RAM). Dein Bestand ergibt andere Zahlen.</sub>

Unter Linux und macOS startet `recall` einen kleinen Hintergrunddienst, der das Modell geladen hält und spätere Suchen über einen Socket beantwortet, den nur dein Konto öffnen kann. Die Ladezeit fällt dann einmal an und nicht bei jeder Suche. Er beendet sich, wenn nichts zu tun ist, und sofort, wenn sich das Programm, `cyberbrain.toml`, deine `waivers.toml` oder das Modell-Manifest ändert. `CYBERBRAIN_NO_DAEMON=1` schaltet ihn ab; unter Windows lädt jede Suche das Modell selbst. `cyberbrain status` zeigt ihn in der Zeile `daemon:`.

## Ein Transformer über ein lokales Ollama

Statische Einbettungen kennen keinen Zusammenhang und keine Wortfolge. Läuft [Ollama](https://ollama.com) auf dem Rechner, findet ein Transformer-Einbettungsmodell spürbar mehr. Das README von 0.8.4 nennt für 34 Fragen an 1.800 deutsche Projektnotizen: die richtige Notiz unter den ersten drei bei 79 % mit potion-multilingual-128M und bei 88 % mit `embeddinggemma:300m-qat-q4_0` (245 MB). Der Preis sind etwa 20 ms je Suche statt weniger, und ein kompletter Neuaufbau des Index dauert Minuten statt Sekunden.

```console
$ ollama pull embeddinggemma:300m-qat-q4_0   # du lädst es herunter
$ cyberbrain bench-embedding                 # misst, was auf diesem Rechner installiert ist
```

`bench-embedding` misst jedes installierte Modell an einer Stichprobe deiner eigenen Abschnitte und gibt den Abschnitt `[embedding]` für das passende aus (`backend = "ollama"` und eine Tabelle `[embedding.ollama]`). Darin steht der Hash der Gewichte, die Ollama lädt. Er übernimmt die Rolle des Manifests: Ändern sich die Gewichte hinter einem Namen, lehnt Cyberbrain sie ab, statt ihre Vektoren in den Index zu mischen.

Die Ollama-Adresse durchläuft dieselbe Prüfung wie jeder Inferenzdienst (Loopback oder privates Netz), jede Anfrage ist eine Audit-Zeile, und kein Hook ruft Ollama auf. Antwortet Ollama nicht, sucht `recall` nach Wörtern und sagt das; `scan` indiziert die Notiz ohne Vektoren, und der nächste Lauf ergänzt sie.

## Der Inferenzdienst

Die optionale Sprachmodell-Schicht spricht mit jedem OpenAI-kompatiblen Dienst: Ollama, LM Studio, llama.cpp, vLLM, llamafile, NVIDIA PAIR. Sie prüft Treffer auf Widersprüche, fasst eine Sitzung zusammen (`propose-session`) und schlägt Ring und Tags vor. Alles Wesentliche funktioniert ohne sie.

```toml
[inference]
base_url = "http://127.0.0.1:11434/v1"
model = "qwen3:8b"            # ohne Modell wird der Dienst nie angesprochen
timeout_ms = 30000
contradiction_budget_ms = 3000
```

Regeln, die nicht nachgeben:

- Die Adresse muss auf Loopback oder ein privates Netz zeigen. Eine öffentliche Adresse oder `100.64.0.0/10` (Carrier-Grade-NAT, auch der Bereich von Tailscale) braucht eine Ausnahme, die du selbst freigibst; siehe [`cyberbrain trust`](../compliance/#ein-geklontes-repository-kann-deine-notizen-nicht-wegschicken).
- Modell, Adresse und Tokenzahlen jedes Aufrufs kommen ins Audit-Log. Viele lokale Server liefern keine Tokenzahlen; dann steht im Log, dass sie fehlten, geschätzt wird nie.
- Das Modell darf nie Ring 0 oder 1 vorschlagen. Ein solcher Vorschlag wird auf Ring 2 herabgesetzt und markiert.
- Ein nicht erreichbarer Dienst schwächt die Funktion und sagt das. Auf einen Anbieter im Netz wird nie ausgewichen.

Laut README dauerte auf einem Server nur mit Prozessor eine einzige Widerspruchsprüfung 126 s, weit über dem Budget von 3 s; die Treffer kamen ungeprüft und mit diesem Hinweis. Diese Schicht ist der einzige Teil, der bessere Hardware will.

### Ein Rechner für das ganze Team

Der Dienst darf auf einem anderen Rechner im eigenen Netz laufen, dann bedient ein Rechner mit Grafikkarte alle. In jedem Store `base_url` auf seine private Adresse setzen. [`docs/SHARED-INFERENCE.md`](https://github.com/bassprofressor-lab/cyberbrain/blob/main/docs/SHARED-INFERENCE.md) (Englisch) beschreibt die Einrichtung. Vor dem bedienenden Rechner steht keine Anmeldung, genau deshalb muss die Adresse privat bleiben.

## Sprache zu Text

`cyberbrain transcribe <datei>` schickt eine Aufnahme an einen [whisper.cpp](https://github.com/ggml-org/whisper.cpp)-Server auf dem eigenen Rechner und schreibt den Text wie `write`: PII-Prüfung, Audit-Zeile, Index. In der Weboberfläche setzt ein Mikrofonknopf neben dem Suchfeld und dem Notizeditor diktierten Text ins Feld; gespeichert wird erst, wenn du speicherst.

```toml
[transcription]
base_url = "http://127.0.0.1:8178"   # whisper-server --convert
language = "de"                      # Vorgabe "de"; jeder whisper-Code oder "auto"
```

Ausgeschaltet, bis `base_url` gesetzt ist, und an dieselbe Adressregel gebunden wie die Inferenz. Im Programm steckt kein C++: Das Modell läuft in einem eigenen Prozess.
