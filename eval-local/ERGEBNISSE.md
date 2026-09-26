# Retrieval-Benchmark — Ergebnisse

Gemessen mit [`scripts/retrieval-bench.py`](../scripts/retrieval-bench.py). Jeder Lauf baut
Wegwerf-Stores (`cyberbrain init`, Notizdateien, `scan --full`) und fragt sie über das echte
Binary ab. Lexikalisch und hybrid unterscheiden sich nur in `embedding.model_path`; jede
Antwort wird am Vermerk „hits are lexical only“ geprüft, ein Lauf im falschen Modus bricht ab
(gegen ein Modellverzeichnis ohne Gewichte geprüft: Abbruch bei der ersten Frage). Keine
LLM-Aufrufe, keine Widerspruchsprüfung, keine Kosten.

Kennzahlen je **Notiz** (eine LongMemEval-Sitzung = eine Notiz; mehrere Blöcke derselben Notiz
zählen einmal), Recall mit `-n 100` Blöcken:
r@k = mindestens eine richtige Notiz unter den ersten k · rall@10 = alle richtigen unter den
ersten 10 · MRR = Kehrwert des Rangs der ersten richtigen · nDCG@10 binär.

## Lauf 26.09.2026

| | |
|---|---|
| Datum | 2026-09-26, 06:42 UTC (LongMemEval) · 06:44 UTC (Store) |
| Commit | `b29f60c` (Zweig `agent-zurechnung`) + Skript aus `punkt3-bench` |
| Binary | `cyberbrain 0.6.1`, `cargo build --release` |
| Modellprofil | `m2v-mean-d256-525239dddeeac4f5` (model2vec `potion-multilingual-128M`, 256 Dim.) |
| Hardware | AMD EPYC-Milan, 12 Kerne (VM), 23 GiB RAM, Linux 6.8; LongMemEval mit 5 parallelen Jobs, Latenz ohne Parallellast |

### LongMemEval-S (öffentlich)

Datensatz: `xiaowu0162/longmemeval-cleaned`, Datei `longmemeval_s_cleaned.json`
(sha256 `d6f21ea9…c3a442`), MIT-Lizenz. 500 Fragen, davon 30 Enthaltungsfragen (`_abs`)
ausgeschlossen, weil es dort nichts zu finden gibt → **470 Fragen**. Je Frage ein eigener Store
aus ihrem Heuhaufen (~50 Sitzungen, ~115k Token), Relevanz = `answer_session_ids`.
Laufzeit gesamt ≈ 6 min.

| Fragetyp | n | Modus | r@1 | r@3 | r@5 | r@10 | rall@10 | MRR | nDCG@10 |
|---|---:|---|---:|---:|---:|---:|---:|---:|---:|
| **alle** | 470 | hybrid | **0,843** | **0,934** | **0,966** | **0,985** | **0,932** | **0,895** | **0,895** |
| | | lexikalisch | 0,843 | 0,932 | 0,960 | 0,979 | 0,898 | 0,892 | 0,883 |
| knowledge-update | 72 | hybrid | 0,972 | 0,986 | 0,986 | 1,000 | 0,972 | 0,981 | 0,970 |
| | | lexikalisch | 0,972 | 1,000 | 1,000 | 1,000 | 0,972 | 0,986 | 0,972 |
| multi-session | 121 | hybrid | 0,860 | 0,950 | 0,975 | 0,992 | 0,893 | 0,910 | 0,891 |
| | | lexikalisch | 0,810 | 0,942 | 0,975 | 0,992 | 0,802 | 0,881 | 0,842 |
| single-session-assistant | 56 | hybrid | 0,929 | 0,964 | 0,964 | 1,000 | 1,000 | 0,948 | 0,960 |
| | | lexikalisch | 0,982 | 1,000 | 1,000 | 1,000 | 1,000 | 0,991 | 0,993 |
| single-session-preference | 30 | hybrid | 0,433 | 0,600 | 0,800 | 0,867 | 0,867 | 0,571 | 0,637 |
| | | lexikalisch | 0,400 | 0,633 | 0,733 | 0,867 | 0,867 | 0,547 | 0,619 |
| single-session-user | 64 | hybrid | 0,828 | 0,984 | 1,000 | 1,000 | 1,000 | 0,908 | 0,931 |
| | | lexikalisch | 0,938 | 0,969 | 0,984 | 0,984 | 0,984 | 0,956 | 0,962 |
| temporal-reasoning | 127 | hybrid | 0,819 | 0,929 | 0,969 | 0,984 | 0,898 | 0,879 | 0,871 |
| | | lexikalisch | 0,795 | 0,906 | 0,945 | 0,969 | 0,866 | 0,857 | 0,844 |

Lesart: Auf LongMemEval-S trägt die lexikalische Suche fast alles; die Fragen teilen meist
Wörter mit der Antwortsitzung. Hybrid gewinnt dort, wo mehrere Sitzungen gefunden werden
müssen (multi-session rall@10 +0,09, temporal +0,03) und verliert r@1 bei den
Einzelsitzungsfragen (single-session-user −0,11, -assistant −0,05): die Vektoren ziehen
thematisch ähnliche, aber falsche Sitzungen nach oben. Das ist ein Retrieval-Wert auf
Sitzungsebene, **keine QA-Genauigkeit** — mit den LongMemEval-Werten der Anbieter (etwa Mem0 94,4,
Supermemory 81,6), die Antwortgenauigkeit mit einem LLM-Leser messen, nicht vergleichbar.

### Eigener Store (Kopie des Arbeitsstores, 1.522 Notizen, 34 Fragen)

Fragenset `eval-local/queries.toml` (lokal, nicht im Repo: Fragen und Notiznamen sind intern).
Veröffentlicht werden nur die Zahlen. Selbst geschrieben, in Kenntnis des Systems — ein
Rauchtest, kein neutraler Benchmark (siehe `PREREG.md`, ebenfalls lokal).

| Gruppe | n | Modus | r@1 | r@3 | r@5 | r@10 | MRR |
|---|---:|---|---:|---:|---:|---:|---:|
| **alle** | 34 | hybrid | 0,500 | **0,794** | 0,882 | 0,882 | **0,667** |
| | | lexikalisch | 0,500 | 0,676 | 0,794 | 0,941 | 0,624 |
| literal | 8 | hybrid | 0,500 | 0,750 | 0,750 | 0,750 | 0,631 |
| | | lexikalisch | 0,250 | 0,625 | 0,750 | 0,875 | 0,454 |
| paraphrase | 13 | hybrid | 0,615 | 0,923 | 1,000 | 1,000 | 0,785 |
| | | lexikalisch | 0,692 | 0,846 | 0,846 | 1,000 | 0,773 |
| morphology | 7 | hybrid | 0,286 | 0,714 | 1,000 | 1,000 | 0,548 |
| | | lexikalisch | 0,571 | 0,571 | 1,000 | 1,000 | 0,679 |
| cross-lingual | 6 | hybrid | 0,500 | 0,667 | 0,667 | 0,667 | 0,599 |
| | | lexikalisch | 0,333 | 0,500 | 0,500 | 0,833 | 0,464 |

Mit `--unit block -n 10` (die Zählweise von `scripts/recall-eval.py`): hybrid r@1/r@3/MRR
0,50 / 0,79 / 0,656, lexikalisch 0,50 / 0,68 / 0,618. Die Analyse vom 25.09. hatte mit
demselben Set 0,56 / 0,82 / 0,700 gegen 0,53 / 0,71 / 0,639 — der Store ist seither
gewachsen, die Differenz hybrid − lexikalisch bei r@3 (+0,11 bzw. +0,12) ist gleich
geblieben. Bei n = 34 ist eine Frage 0,03; kleinere Bewegungen sind Rauschen.

### Latenz (Wandzeit je `recall`, Prozessstart eingeschlossen, 20 Anfragen)

| Pfad | LongMemEval-Store (~50 Notizen) | eigener Store (1.522 Notizen) |
|---|---:|---:|
| CLI kalt, lexikalisch (`CYBERBRAIN_NO_DAEMON=1`) | p50 9,3 ms · p95 11,3 ms | p50 13,8 ms · p95 16,8 ms |
| CLI kalt, hybrid (Modell je Aufruf laden) | p50 1.500 ms · p95 1.564 ms | p50 1.473 ms · p95 1.627 ms |
| erster Aufruf ohne Dienst (antwortet selbst, startet Dienst) | 1.455 ms | 1.485 ms |
| bis der Dienst annimmt | 1,47 s | 1,51 s |
| **über den Dienst, warm, hybrid** | **p50 7,8 ms · p95 8,5 ms** | **p50 13,6 ms · p95 16,8 ms** |

Über den Dienst kostet hybrid so viel wie lexikalisch ohne Dienst; der kalte Pfad ist fast
ausschließlich Modell-Laden.

## Nachmessen

```bash
# einmalig: Datensatz in einen Cache außerhalb des Repos (277 MB, MIT)
mkdir -p ~/.cache/cyberbrain-eval/longmemeval
curl -L -o ~/.cache/cyberbrain-eval/longmemeval/longmemeval_s_cleaned.json \
  https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_s_cleaned.json

cargo build --release -p cyberbrain
# --model-dir: Verzeichnis mit model.safetensors, tokenizer.json, manifest.json (eine Kopie genügt)
# --work: kurzer Pfad, der Dienst-Socket <work>/lme/<id>/daemon.sock muss in sockaddr_un passen
scripts/retrieval-bench.py longmemeval --data ~/.cache/cyberbrain-eval/longmemeval/longmemeval_s_cleaned.json \
  --model-dir /pfad/zu/models/model2vec --work /tmp/cbb --jobs 5 --out lme.json
# Stichprobe zum Ausprobieren: --limit 20

# eigener Store (nur notes/ wird kopiert, Konfiguration und Index nicht)
scripts/retrieval-bench.py store --source ~/projekt/.cyberbrain --queries eval-local/queries.toml \
  --model-dir /pfad/zu/models/model2vec --work /tmp/cbs --out store.json
```

## Bewusst nicht gemessen

- **LongMemEval-V2** (arXiv 2605.12493, Apache-2.0): geprüft, nicht geeignet. Es gibt keine
  Beleg-Labels je Trajektorie, bewertet wird nur die Antwort eines LLM-Lesers (Qwen3.5-9B),
  Heuhaufen bis 115 M Token, multimodal (Screenshots), 7,1 GB. Ohne LLM-Aufrufe ist dort
  kein Retrieval-Wert zu bilden.
- **LongMemEval-M** (~500 Sitzungen je Frage) und die Zug-Ebene (`has_answer` je Turn):
  möglich mit demselben Skript, nicht gelaufen.
- **QA-Genauigkeit** jeder Art: bräuchte einen Leser und einen Richter.
