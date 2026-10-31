---
layout: ../../layouts/Doc.astro
title: "Team-Hub"
description: "Ein Rechner sammelt die Nachweise der übrigen: Lizenz und Plätze, Betrieb unter Linux oder als Windows-Dienst, TLS mit festgelegtem Zertifikat, Anmeldung der Rechner, Notizen teilen nach Bereich mit Gegenzeichnung, Rollen, Sicherung, Aufbewahrung und Meldungen."
lang: de
slug: "hub/"
---

# Team-Hub

Ein Team behält seine Notizen, wo sie immer waren: im Store jeder Person. Ein Hub (im README „Zentraleinheit“) ist der eine Rechner, der die **Nachweise** einsammelt: welcher Rechner wann welche Art von Handlung vorgenommen hat, in Ketten, die sich nachträglich nicht unbemerkt ändern lassen. Eine Zeile sagt, dass eine Notiz geschrieben wurde, nie, was darin stand.

Der Hub ist eine eigene Oberfläche, nicht `serve` mit offener Adresse. Er verlangt eine Anmeldung, und was er kann, ist kurz: Audit-Zeilen annehmen und, wo ein Store es eingeschaltet hat und zwei Menschen einem Bereich zugestimmt haben, die Notizen dieses Bereichs halten und zurückgeben. In den Store eines anderen kann er weder lesen noch schreiben, nichts löschen und keine Frist anwenden.

Diese Seite verdichtet [`docs/HUB.md`](https://github.com/bassprofressor-lab/cyberbrain/blob/main/docs/HUB.md) (Englisch), wo alles ausführlich steht. Befehle und Optionen hier sind an 0.8.4 geprüft.

## Die Lizenz

Ein Hub braucht eine, ohne sie wird nichts registriert und nichts gesammelt. Sie ist eine signierte Datei, geprüft gegen einen Schlüssel im Programm. Nach Hause telefoniert nichts.

```console
$ cyberbrain hub licence install licence.txt --data /var/lib/cyberbrain/hub.db
$ cyberbrain hub licence show --data /var/lib/cyberbrain/hub.db
```

`licence show` endet nicht mit 0, wenn der Hub nicht sammelt; eine Überwachung ist damit eine Zeile. Eine Lizenzdatei neben dem Datenbestand (`licence.txt`) wird beim Start übernommen.

**Plätze sind Rechner.** Jedes Projekt ist ein eigener Store und damit ein eigenes Gerät am Hub, aber Geräte desselben Rechners teilen sich einen Platz. Ein weiteres Projekt auf einem Rechner, der schon einen Platz hat, lässt sich bei voller Lizenz registrieren, wenn man den Rechner nennt: `hub add ws-021-dispo --machine ws-021`.

**Läuft sie ab**, nimmt der Hub keine Zeilen mehr an und tut sonst nichts. Der Bestand bleibt lesbar, die Geräte arbeiten weiter, die Clients puffern, und eine erneuerte Lizenz übernimmt, was sie gehalten haben, ohne Lücke in der Kette.

## Einen betreiben

Unter Linux:

```console
$ cyberbrain hub add "laptop-anna" --data /var/lib/cyberbrain/hub.db
$ cyberbrain hub serve --addr 0.0.0.0:7788 --data /var/lib/cyberbrain/hub.db --tls-generate
```

`hub add` gibt das Geräte-Token einmal aus und speichert nur einen Hash. Mit `--invite <datei> --hub-url <url>` entsteht stattdessen eine Einladungsdatei, die alles enthält, was der Rechner braucht (auf Wunsch mit `--inference-url` auch die Adresse des gemeinsamen Inferenzdienstes). Weitergeben wie ein Passwort. `hub serve` lauscht von Haus aus auf `127.0.0.1:7788`; ein Hub, den nur sein eigener Rechner erreicht, sammelt nichts, also eine Adresse angeben, die das Netz erreicht. HUB.md enthält eine systemd-Unit.

Unter Windows im Installationsprogramm **Hub service (collector)** ankreuzen oder `cyberbrain-setup.exe /S /HUB` ausführen. Das registriert den Dienst *Cyberbrain Hub* auf `0.0.0.0:7788`, öffnet den Port in der Windows-Firewall, erzeugt ein eigenes Zertifikat und legt `C:\ProgramData\Cyberbrain` an. Lizenzieren heißt dann, `licence.txt` dorthin zu kopieren und den Dienst neu zu starten. Dasselbe von Hand:

```console
> cyberbrain hub service install --data C:\ProgramData\Cyberbrain\hub.db
> cyberbrain hub service status
```

`hub service uninstall` und das Entfernen des Programms lassen den Datenbestand stehen.

### Viele Rechner auf einmal

Eine Sammeleinladung meldet viele Projekte mit einer Datei an:

```console
$ cyberbrain hub invite create --uses 60 --expires P14D --label "Rollout Disposition" \
    --hub-url https://hub.example.internal:7788 --out rollout.json --data /var/lib/cyberbrain/hub.db
```

Jedes Projekt, das `cyberbrain hub enrol rollout.json` ausführt (oder im Windows-Starter *Mit dem Firmen-Hub verbinden…* wählt), bekommt ein eigenes Gerät. `--expires` ist höchstens 90 Tage; verwaltet wird mit `hub invite list` und `hub invite revoke`. Eine Adresse, die binnen zehn Minuten zehn unbekannte Codes probiert, wird für den Rest dieser Zeit mit `429` abgewiesen. Hinter einem Reverse Proxy haben alle Rechner die Adresse des Proxys, ein einziger Rater sperrt dort also alle für diese Zeit. Die Einführung im Unternehmen steht in [`docs/ROLLOUT.md`](https://github.com/bassprofressor-lab/cyberbrain/blob/main/docs/ROLLOUT.md).

## Verschlüsselung

Ohne eigene Zertifizierungsstelle erzeugt `--tls-generate` einmal ein Zertifikat und legt es neben den Bestand. Jede danach ausgestellte Einladung trägt seinen Fingerabdruck, und angemeldete Rechner **legen sich darauf fest**: Sie nehmen dieses Zertifikat und kein anderes und lehnen reines http ab. Browser kennen es nicht und warnen; `hub cert show` gibt den Fingerabdruck und die Befehle aus, mit denen es in den Zertifikatspeicher eines Rechners kommt, `hub cert export <datei>` kopiert das Zertifikat (nie den Schlüssel) für eine Freigabe oder eine Gruppenrichtlinie.

Mit einem eigenen Zertifikat `--tls-cert` und `--tls-key` im PEM-Format angeben. Darauf legt sich kein Client fest; er prüft es gegen den Zertifikatspeicher des Rechners, eine Erneuerung hält ihn also nicht an.

Ganz ohne Zertifikat sammelt der Hub weiter, warnt bei jedem Start und zeigt ein Banner. Das Administratorpasswort lässt sich dann nur am Rechner des Hubs selbst eingeben.

## Die Seite des Hubs und die Anmeldung

Die Seite des Hubs liegt auf seinem eigenen Rechner unter `http://localhost:7788/`: Lizenz, Plätze, Geräte, Personen mit Rolle, Sammeleinladungen. Sie wird auf dem Server erzeugt und hat kein JavaScript.

Ein voreingestelltes Passwort gibt es nicht. Das Konto heißt `admin`, und **der erste Besuch vom Rechner des Hubs aus** legt das Passwort fest. Danach öffnet sich die Seite von jedem Arbeitsplatz, sofern der Hub verschlüsselt. `hub admin reset --data …` vergisst das Passwort, und der nächste Besuch vom Rechner des Hubs legt ein neues fest. Ein falsches Passwort kostet eine feste Wartezeit statt einer Sperre, und jede abgelehnte Anmeldung steht im eigenen Log des Hubs.

„Vom Rechner des Hubs aus“ heißt: eine Verbindung über Loopback, deren `Host`-Kopfzeile ebenfalls Loopback nennt. So kann eine Seite mit DNS-Rebinding einen Hub ohne Passwort nicht für sich beanspruchen.

> **Offener Punkt in 0.8.4: ein Reverse Proxy und die erste Anmeldung.** Ein Reverse Proxy auf dem Rechner des Hubs verbindet sich über Loopback, und je nachdem, welchen `Host` er weitergibt, kann die Regel für den ersten Besuch falsch beurteilen, ob eine Anfrage vom Rechner selbst kommt. Wie der Hub diesen Fall behandeln soll, ist noch nicht entschieden. Bis dahin das Administratorpasswort direkt am Rechner festlegen, unter `http://localhost:7788/` und nicht über den Proxy, bevor ein Proxy davorkommt, und den Proxy den ursprünglichen `Host` weitergeben lassen. Bei nginx ist das `proxy_set_header Host $host;`. Ohne diese Zeile schickt nginx die Adresse des Hubs, `127.0.0.1:7788`, und die sieht aus wie der Rechner selbst.

## Nachweise liefern

```console
$ cyberbrain hub enrol ws-021.json
$ cyberbrain hub push
```

`hub push` schickt die Audit-Zeilen des Stores an den Hub, bei dem er angemeldet ist. Über einen Zeitplan ausführen, einmal pro Stunde reicht; der Windows-Starter liefert von selbst alle Viertelstunde. Gepuffert wird nicht extra: Das Audit-Log ist der Puffer, der Hub überspringt, was er schon hat, und eine Lieferung mit Lücke wird mit `409` abgelehnt. Überschneidungen sind kein Problem.

Das Geräte-Token liegt nicht im Store, sondern im eigenen Konfigurationsverzeichnis, eine Datei je Store und Hub, oder für ein Dienstkonto in `CYBERBRAIN_HUB_TOKEN`. Wer die Hub-Adresse in `cyberbrain.toml` ändert, lenkt die Nachweise nicht um: Die Ausgangsprüfung lehnt jedes Ziel außer dem Hub ab, bei dem der Store angemeldet wurde.

`hub fleet` zeigt die Geräte, die mit Problemen zuerst: nie geliefert, Lieferung abgelehnt, seit 71 Stunden still, ältere Version.

## Notizen teilen

Von Haus aus hält der Hub Audit-Zeilen und sonst nichts. Der Text einer Notiz geht nur, wenn **alles** davon zutrifft:

| | |
|---|---|
| `allow_note_sync = true` im Abschnitt `[hub]` des Stores | von Haus aus aus |
| die Notiz hat einen `bereich` | ohne Bereich wird nicht geteilt |
| Ring 2, 3 oder 4 | Ring 0 und 1 verlassen den Rechner nie, egal was freigegeben ist |
| eine Freigabe für dieses Gerät und diesen Bereich, in der richtigen Richtung | `send`, `receive` oder `both` |
| eine zweite Person hat die Freigabe gegengezeichnet | nicht die, die sie geschrieben hat |

```console
$ cyberbrain hub grant add --device dev_01J… --bereich disposition --direction both \
    --reason "Schichtübergabe innerhalb der Abteilung"
$ cyberbrain hub grant approve bg_01J… --as <zugangsdaten der gegenzeichnenden person>
```

Zwei Menschen, weil wer den Hub betreibt, Geräte registriert und ein Geräte-Token aus einer Einladungsdatei lesen kann. Könnte dieselbe Person auch einen Bereich auf ein Gerät richten, wäre jeder Notiztext auf dem Hub nur ein Formular von ihr entfernt.

Auf dem Rechner schickt `cyberbrain hub push --notes` die Notizen, und `cyberbrain hub pull` holt, was der Hub für diesen Rechner hat. `pull` überschreibt nie eine Notiz, die hier seit dem letzten Abruf geändert wurde, sondern meldet sie. Löschungen werden gemeldet und nur mit `--apply-erasures` umgesetzt. Ändern zwei Rechner dieselbe Notiz, entsteht ein Konflikt, den eine Person mit der Rolle `editor` für diesen Bereich mit `hub conflicts` entscheidet.

`cyberbrain hub erase <name> --bereich <b>` löscht die Kopie auf dem Hub und hinterlässt einen Grabstein. Das geht auch, wenn das Teilen inzwischen abgeschaltet ist. Ein lokales `forget` erreicht den Hub nicht; es sagt das und nennt diesen Befehl.

## Wer was sehen darf

| Rolle | sieht | sieht nicht |
|---|---|---|
| `admin` | Geräte, Lücken, Versionen, Plätze, Lizenz | Aktivitätszeilen |
| `auditor` | Aktivität, nur in einem freigegebenen Zeitfenster | nichts ohne Gegenzeichnung |
| `countersigner` | jede Anfrage mit Grund, das Zugriffslog, wartende Freigaben | Zeilen, außer bei zusätzlicher Rolle `auditor` |
| `editor` | Notizkonflikte in den zugewiesenen Bereichen | alles andere |

```console
$ cyberbrain hub principal add "M. Kraus" --role auditor --expires P1Y
$ cyberbrain hub request --device dev_01M1Y9… --reason "Revisionsanfrage vom 5. September" --as $AUDITOR
$ cyberbrain hub approve req_01M1YB… --hours 2 --as $BETRIEBSRAT
$ cyberbrain hub disclose req_01M1YB… --out-dir fall-2026-09 --as $AUDITOR
```

Zugangsdaten werden einmal gezeigt und als Hash gespeichert. Jeder Schritt steht im eigenen Log des Hubs mit eigener Hashkette, `hub access-log`. Personen mit der Rolle `countersigner` oder `admin` entstehen nur am Rechner mit `hub principal add`, nie über die Seite.

**Was das nicht leistet.** Es erzwingt den Weg: Über dieses Programm ist Aktivität nur mit einer Anfrage erreichbar, die jemand anderes freigegeben hat. Wer Dateizugriff auf die Datenbank des Hubs hat, kann sie mit jedem SQLite-Werkzeug öffnen. Den Hub deshalb auf einen Rechner mit kontrolliertem Zugang stellen, und in einer Betriebsvereinbarung von einem Verfahren mit Software-Unterstützung sprechen, nicht von einer Garantie der Software.

## Pflege

| Befehl | |
|---|---|
| `hub verify` | jede Kette gegen die Zeilen prüfen, wie sie jetzt auf der Platte liegen; nicht 0 bei einer gebrochenen Kette |
| `hub backup <datei>` | eine wiederherstellbare Kopie über SQLite, nach dem Schreiben geprüft; überschreibt nie. `hub.db` nicht von Hand kopieren: Die neuesten Zeilen können noch in `hub.db-wal` liegen. |
| `hub retention set P2Y` | die Frist; entfernt von sich aus nichts |
| `hub retention propose --reason …` / `approve <id>` | eine Bereinigung, von einer Person aufgeschrieben und ausgeführt, wenn eine gegenzeichnende Person unterschreibt |
| `hub alerts set <url>` | was einen Menschen braucht, an einen Webhook melden: wartende Gegenzeichnungen, auslaufende Lizenz, seit 48 Stunden stille Geräte, fehlschlagende Governance-Aufrufe, benutzte Köder. Nur Loopback oder privat, außer mit `--allow-public` |
| `hub report --out-dir <verz>` | was die Geräte in einem Zeitraum getan haben, als Zustand: eine Zeile je Gerät mit Zeilenzahl und ob seine Kette hält. Nicht die Zeilen selbst; die gibt es nur über `request`, `approve` und `disclose`. |
| `hub revoke <id>` | ein Gerät vom Senden ausschließen; seine Zeilen bleiben |

Sicherungen behalten, was eine spätere Bereinigung entfernt. Ist die Frist ein Versprechen an die Menschen, um deren Aktivität es geht, müssen die Sicherungen in derselben Frist rotiert werden.
