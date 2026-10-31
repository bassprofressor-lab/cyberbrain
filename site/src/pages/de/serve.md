---
layout: ../../layouts/Doc.astro
title: "Weboberfläche und HTTP-API"
description: "cyberbrain serve: die eingebaute Weboberfläche und HTTP-API, nur auf Loopback und ohne Anmeldung, das Terminal in der Seite, Schreiben in Ring 0 und 1 und der Starter unter Windows."
lang: de
slug: "serve/"
---

# Weboberfläche und HTTP-API

```console
$ cyberbrain serve
cyberbrain serve: http://127.0.0.1:7777/  (loopback only, this account only; API at /api/v1)
```

Die Seite ist ins Programm kompiliert, spricht Deutsch und Englisch und lädt nichts aus dem Netz. Sie bietet die Suche, Notizen mit ihren Zitaten und Links, Freigaben für Vorschläge (mit Zeilenvergleich, wenn ein Vorschlag eine vorhandene Notiz ändert), einen Graphen der Notizen nach Ring, den Zustand von Store, Modell und Inferenzdienst, die Compliance-Ansicht mit Ausgangsregister und Pflichtenkatalog und die Übersicht, was mit einem Hub geteilt würde. Dazu hat sie eine eigene Kommandozeile, die dasselbe Programm gegen denselben Store ausführt und Standardausgabe, Fehlerausgabe und Rückgabewert zurückgibt.

Die API liegt unter `/api/v1`, zum Beispiel `GET /api/v1/status`. Die [Spezifikation](https://github.com/bassprofressor-lab/cyberbrain/blob/main/docs/SPEC.md) listet die Routen in §8.1.

## Optionen

| Option | |
|---|---|
| `--port <PORT>` | von Haus aus 7777 |
| `--no-open` | keinen Browser öffnen |
| `--terminal` | ein echtes Terminal in der Seite anbieten, siehe unten |
| `--allow-resident-writes` | diesem Server das Schreiben in Ring 0 und 1 erlauben |

## Warum es keine Anmeldung gibt

`serve` lauscht nur auf Loopback. Für das Netz lässt es sich nicht öffnen: Es liest, schreibt, löscht und wendet Aufbewahrungsfristen an, und anzumelden gibt es nichts, weil es keinen Zugriff von außen gibt. Für mehrere Rechner gibt es den [Hub](../hub/), eine eigene Oberfläche mit Anmeldung.

Drei weitere Grenzen stützen das:

- **Nur das eigene Konto.** Unter Linux kann sich nur dein Konto (und root) verbinden.
- **Nur unter dem eigenen Namen.** Jede Route antwortet nur unter `127.0.0.1:<port>`, `localhost:<port>` und `[::1]:<port>`, alles andere bekommt `421 Misdirected Request`. Das verhindert DNS-Rebinding, bei dem eine Webseite ihren eigenen Namen auf 127.0.0.1 umbiegt und die API dann als ihre eigene liest. Es heißt auch: Ein Alias in `/etc/hosts` oder ein vorgeschalteter Reverse Proxy funktioniert nicht, und ein SSH-Tunnel muss dieselbe Portnummer weiterleiten.
- **Ring 0 und 1 bleiben am Terminal.** Ohne `--allow-resident-writes` schreibt der Server sie nicht, weil jedes Programm auf dem Rechner, auch ein Agent, ihn ohne Zugangsdaten erreichen könnte.

```console
$ curl -s -o /dev/null -w '%{http_code}\n' -H 'Host: evil.example' http://127.0.0.1:7777/
421
```

## Das Terminal

`cyberbrain serve --terminal` setzt eine echte Shell in die Seite: `ssh`, einen Agenten, alles, was man an einer Eingabeaufforderung startet, im Verzeichnis des Projekts. Ohne ausdrücklichen Wunsch ist es aus, weil jede andere Route Notizen liest oder schreibt und diese einen Prozess startet. Ist es an, trägt die beim Start ausgegebene Adresse ein Token in ihrem Fragment; nur diese Adresse öffnen. Ein Verbindungsaufbau von einem anderen Ursprung wird abgelehnt.

Das Ausgangsregister führt das Terminal als den einen Weg, den dieses Programm nicht vermittelt. Was darin läuft, gehört dir, läuft in deinem Namen und wird von Cyberbrain weder erlaubt noch festgehalten. Es steht im Register, damit niemand das Register für vollständig hält, während eine Shell einen Tastendruck entfernt ist.

## Ohne Oberfläche

Ein Programm, das mit `--no-default-features` gebaut wurde, hat keine Seite. `serve` beantwortet dann die API und sagt, dass die Seite nicht eingebaut ist.

## Unter Windows

Das [Installationsprogramm](../install/#windows-ohne-terminal) bringt einen Starter mit, der `serve` für ein Projekt auf einem Port startet, den Windows aussucht, und die Seite öffnet, im eigenen Fenster oder im Browser. Er bleibt im Infobereich, bis du ihn beendest, und mit ihm endet der Server.
