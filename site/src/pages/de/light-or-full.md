---
layout: ../../layouts/Doc.astro
title: "Light oder voll"
description: "Wann Cyberbrain Light reicht und wann das volle Cyberbrain mit Bedeutungssuche, Store-Schutz, Vorschlägen, Team-Hub und Nachweisen für DSGVO und KI-Verordnung besser passt."
lang: de
slug: "light-or-full/"
---

# Light oder voll

[Cyberbrain Light](https://bassprofressor-lab.github.io/cyberbrain-light/de/) ist ein eigenes, kleineres Programm (`cbl`) für denselben Aufbau des Stores. Es behält Ringe und Zitate und lässt fast alles andere weg.

|  | [Light](https://bassprofressor-lab.github.io/cyberbrain-light/de/) | Cyberbrain |
|---|---|---|
| Suche | Stichwörter (BM25) | Stichwörter **und** Bedeutung, kombiniert |
| Modell | keins | ein Einbettungsmodell, das du selbst ablegst, oder ein lokales Ollama |
| Widerspruchsprüfung | nein | gegen einen lokalen Inferenzdienst |
| Hooks für Claude Code | Sitzungsstart | Sitzungsstart, Store-Schutz vor jedem Werkzeugaufruf, Eingabe, Verdichtung |
| Ring 0 und 1 | am Terminal geschrieben | am Terminal geschrieben; Agenten schlagen vor, ein Mensch prüft |
| Nachweise für DSGVO und KI-Verordnung | nein | Ausgangsregister, Löschung mit Bericht, Auskunft, signierte Audit-Kette, PII-Prüfung, Pflichtenkatalog |
| Team | nein | Hub für Nachweise, Notizen teilen auf Wunsch |
| Weboberfläche | nein | im Programm eingebaut, Deutsch und Englisch |
| Netzwerkzugriff | keiner, es gibt keinen Code dafür | nur die Wege aus `policy egress`, jeder aus, bis er eingerichtet ist |
| Programm (Linux) | 5,0 MB | 20 MB (0.8.4) |

<sub>Die Größe von Light stammt aus dessen eigener Dokumentation (gemessen am 06.09.2026). Die von Cyberbrain ist die Größe der Release-Datei von 0.8.4.</sub>

**Light passt**, wenn eine Person oder ein kleines Team will, dass der Agent sich Projektwissen merkt, und in den Notizen keine personenbezogenen Daten stehen: Bauschritte, Entscheidungen, Fehler und ihre Lösung. Es läuft überall und kann nichts über das Netz schicken.

**Das volle Cyberbrain passt**, wenn die Suche Bedeutung finden soll und nicht nur Wörter, wenn ein Agent im Projekt arbeitet und seine eigenen Regeln nicht umschreiben darf, wenn mehrere Rechner beteiligt sind, oder wenn im Store personenbezogene Daten liegen können und du zeigen musst, was gespeichert wurde, wer es geschrieben hat und dass eine Löschung wirklich passiert ist.

> Beide können denselben Store nutzen. Light führt die Prüfungen des vollen Produkts aber nicht aus: Eine über Light geschriebene Notiz umgeht PII-Prüfung und Audit-Zeile, und `cbl forget` hinterlässt keinen Löschnachweis. In einem Store, der darauf angewiesen ist, nur über `cyberbrain` schreiben.

[Cyberbrain auf krynexlabs.de](https://www.krynexlabs.de/cyberbrain/)
