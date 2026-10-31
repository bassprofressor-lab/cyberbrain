---
layout: ../../layouts/Doc.astro
title: "Installation"
description: "Cyberbrain von crates.io installieren, als geprüftes Release-Programm für Linux, Windows und macOS, oder mit dem Windows-Installationsprogramm samt Starter im Infobereich."
lang: de
slug: "install/"
---

# Installation

Paket und Programm heißen beide `cyberbrain`.

## Über crates.io

```console
$ cargo install cyberbrain
$ cyberbrain --version
cyberbrain 0.8.4
```

Dafür braucht es Rust 1.98 oder neuer. Die Weboberfläche ist ins Programm kompiliert und liegt im Paket, ein node wird also nicht gebraucht. 0.8.3 wurde auf crates.io ohne die Oberfläche veröffentlicht und ließ sich nicht installieren; 0.8.4 behebt genau das und sonst nichts.

Wer die Oberfläche nicht im Programm haben will, lässt sie weg. Kommandozeile, Hooks, MCP-Server und HTTP-API bleiben gleich, und `serve` beantwortet die API und sagt dazu, dass die Seite nicht eingebaut ist:

```console
$ cargo install cyberbrain --no-default-features
```

## Als Release-Programm

Jedes [Release auf GitHub](https://github.com/bassprofressor-lab/cyberbrain/releases/latest) enthält vier Dateien und eine `SHA256SUMS`:

| Datei | für |
|---|---|
| `cyberbrain-linux-x86_64` | Linux, x86_64 |
| `cyberbrain-windows-x86_64.exe` | Windows, x86_64, nur das Programm |
| `cyberbrain-macos-aarch64` | macOS mit Apple-Chip |
| `cyberbrain-setup-0.8.4.exe` | das Windows-Installationsprogramm, siehe unten |

Vor dem Start prüfen:

```console
$ sha256sum -c SHA256SUMS
cyberbrain-linux-x86_64: OK
cyberbrain-macos-aarch64: OK
cyberbrain-setup-0.8.4.exe: OK
cyberbrain-windows-x86_64.exe: OK
$ chmod +x cyberbrain-linux-x86_64
$ ./cyberbrain-linux-x86_64 --version
cyberbrain 0.8.4
```

Zur Laufzeit braucht das Programm nichts: kein SQLite aus dem System, kein OpenSSL, keinen Modellserver, kein node.

## Windows ohne Terminal

`cyberbrain-setup-<version>.exe` installiert das Kommandozeilenwerkzeug und einen kleinen Starter und legt **Cyberbrain** ins Startmenü. Der Starter fragt einmal, welches Projekt er öffnen soll, startet den Server auf einem Port, den Windows aussucht, öffnet die Seite und bleibt im Infobereich. Mit Rechtsklick gibt es den Projektordner, ein anderes Projekt, *Mit Windows starten* (von Haus aus aus) und Beenden. Beenden hält auch den Server an. Ein zweiter Start zeigt die schon laufende Seite statt eines zweiten Symbols. Starter und Installationsprogramm sprechen auf einem deutschen Windows Deutsch, sonst Englisch.

Das Installationsprogramm ist nicht signiert, SmartScreen warnt deshalb beim ersten Start: *Weitere Informationen*, dann *Trotzdem ausführen*. Es trägt `C:\Program Files\Cyberbrain` in den `PATH` der Maschine ein, damit `cyberbrain` auch in einem neuen Terminal läuft.

Für eine Installation ohne Rückfragen gibt es Schalter:

| Befehl | installiert |
|---|---|
| `cyberbrain-setup.exe /S` | einen Arbeitsplatz |
| `cyberbrain-setup.exe /S /HUB` | einen Arbeitsplatz und den Hub-Dienst |
| `cyberbrain-setup.exe /S /INVITE=\\server\rollout\rollout.json` | einen Arbeitsplatz mit Sammeleinladung |

`/HUB` richtet den [Team-Hub](../hub/) als Windows-Dienst ein. `/INVITE` legt eine Sammeleinladung für den Starter ab, der dann je Projekt einmal fragt, ob er verbinden soll. Die Schritt-für-Schritt-Fassung für ein Unternehmen steht in [`docs/ROLLOUT.md`](https://github.com/bassprofressor-lab/cyberbrain/blob/main/docs/ROLLOUT.md) (Englisch).

## Aus einem Checkout

Im Repository wird die Oberfläche gebaut, nicht mitgeliefert. Also zuerst sie bauen:

```console
$ git clone https://github.com/bassprofressor-lab/cyberbrain && cd cyberbrain
$ (cd ui && npm ci && npm run build)
$ cargo install --path crates/cyberbrain
```

## Danach

```console
$ cd dein-projekt
$ cyberbrain init                          # legt hier .cyberbrain/ an
$ cyberbrain install --client claude-code  # trägt die Hooks in .claude/settings.json ein
```

Weiter mit [fünf Minuten](../quickstart/).

## Deinstallieren

```console
$ cyberbrain install --undo     # entfernt nur die Einträge, die cyberbrain geschrieben hat
$ cargo uninstall cyberbrain
```

Die Notizen bleiben, wo sie sind. `.cyberbrain/notes/` ist einfaches Markdown und gehört dir. Unter Windows lässt das Entfernen des Programms auch den Datenbestand eines Hubs stehen.
