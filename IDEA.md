Oxbloom – Projektstand und Agentendefinition
Stand: 23. Oktober 2026 Zweck: Wiederverwendbare Projektübersicht und Arbeitsanweisung für einen Coding-Agenten.
Projektziel
Oxbloom ist ein geplantes Open-Source-MVC-Framework in Rust, inspiriert von Spring Boot. Es soll modular, erweiterbar und automation-first sein.
Architekturprinzipien
● Cargo-Workspace mit kleinen, optionalen Crates.
● Tokio/Hyper oder eine etablierte HTTP-Basis verwenden, keine eigene HTTP-Protokollimplementierung.
● Gemeinsames Routing für alle HTTP-Methoden.
● Compile-time Procedural Macros für Annotationen wie #[controller] und #[get].
● Nur ausgewählte Komponenten und deren notwendige Abhängigkeiten aktivieren.
● Backend und Frontend entkoppeln; React/TypeScript ist optional.
● Security standardmäßig sicher konfigurieren; keine eigene Kryptografie entwickeln.
● Nur als fertig bezeichnen, was tatsächlich implementiert und getestet wurde.
Geplante Komponenten
● oxbloom-core: gemeinsame Typen und HTTP-Methoden.
● oxbloom-di: Dependency Injection und Service-Lebenszyklen.
● oxbloom-macros: Controller- und Route-Attribute.
● oxbloom-web / oxbloom-http: Routing, Requests, Responses und Serverintegration.
● oxbloom-security sowie optionale Auth-, JWT- und Session-Module.
● oxbloom-jobs, oxbloom-queue, oxbloom-worker, oxbloom-scheduler.
● oxbloom-config, oxbloom-observability, oxbloom-test.
● oxbloom-react und frontend/react-app als optionale React-/TypeScript-Integration.
● Neu: oxbloom-ai als optionale LLM-Komponente.
LLM-Komponente: oxbloom-ai
Ziel: Anwendungen sollen austauschbare LLM-Anbieter nutzen können, ohne an einen einzelnen Anbieter gebunden zu sein.
Geplante Fähigkeiten: - Chat-Aufrufe und Streaming. - Konfigurierbarer Anbieter, Modell, Endpunkt und Laufzeitparameter. - Cloud- und lokal betriebene Modelle über Provider-Adapter. - Strukturierte Ausgaben und JSON-Schemata. - Tool Calling mit expliziter Freigabe und kontrollierten Berechtigungen. - Embeddings und optionale RAG-/Retrieval-Integration. - Timeouts, Fehlerbehandlung, begrenzte Wiederholungen und Telemetrie. - Tests mit Mock-Providern, damit Tests keinen echten API-Schlüssel benötigen.
Architekturvorschlag: - Provider-neutrale Traits und Nachrichtentypen in oxbloom-ai. - Anbieterintegrationen als optionale Adapter-Crates oder Cargo-Features. - API-Schlüssel ausschließlich über Umgebungsvariablen oder Secret-Management; nie in den Quellcode oder Logs schreiben. - Keine Anbieterbindung in oxbloom-core. - Tool-Aufrufe nie allein aufgrund einer LLM-Antwort unkontrolliert ausführen; Berechtigungen, Validierung und gegebenenfalls menschliche Freigabe sind Aufgabe der Anwendung. - RAG und Vektordatenbanken später als optionale Integrationen ergänzen.
Bootstrap-CLI: automation-first
Eigenständige oxbloom-cli-Crate mit Befehlen wie:
oxbloom new mein-projekt --web rest --security jwt --database postgres --jobs --tests --ai
oxbloom add ai
oxbloom list
Geplante Optionen: - Web: rest, mvc - Security: jwt, session - Datenbank: postgres, sqlite - Frontend: react - optionale Features: jobs, tests, ai
Das CLI soll: - vollständig per Flags und skriptfähig bedienbar sein; - später optional interaktive Menüs bieten; - ein Komponentenmanifest bzw. eine oxbloom.toml-Konfiguration verwenden; - nur ausgewählte Module und notwendige Abhängigkeiten eintragen; - ein generiertes Projekt erstellen, das tatsächlich kompiliert; - bei gleicher CLI-Version und Konfiguration reproduzierbare Ergebnisse liefern.
Bisheriger Gesprächsstand – noch im Repository verifizieren
Der Nutzer meldete nach frühen Schritten „alles grün”. Die Codevorschläge wurden hier aber nicht unabhängig aus dem Dateisystem geprüft:
● Workspace mit oxbloom-core, oxbloom-di, oxbloom-macros; oxbloom-cli wurde später vorgeschlagen.
● HttpMethod sollte GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS, TRACE, CONNECT sowie Other(String) unterstützen.
● oxbloom-core sollte HttpMethod und InvalidHttpMethod exportieren.
● oxbloom-macros ist als Procedural-Macro-Crate mit syn, quote und proc-macro2 vorgesehen.
● #[controller("/api/users")] erzeugt bislang im Entwurf eine BASE_PATH-Konstante.
● Der Entwurf von #[get("/")] registriert noch keine echte Route.
● oxbloom-cli verwendet im Entwurf clap und anyhow, parst bislang aber nur Optionen und generiert noch keine Projektdateien.
Agentendefinition – zum Einfügen in einen Coding-Agenten
Du bist der technische Coding-Agent für Oxbloom. Erweitere das bestehende Repository in kleinen, nachvollziehbaren und kompilierbaren Schritten.
Vorgehen
1. Untersuche zuerst die tatsächliche Repository-Struktur, Cargo.toml-Dateien, Rust-Toolchain und Tests.
2. Fasse Ist-Zustand, Risiken und offene Fragen kurz zusammen.
3. Wähle einen kleinen, zusammenhängenden Implementierungsschritt.
4. Ändere nur die dafür nötigen Dateien.
5. Implementiere die Funktion real; gib Platzhalter nicht als fertige Features aus.
6. Ergänze Unit-Tests und für öffentliche APIs möglichst Integrationstests.
7. Führe nach Möglichkeit cargo fmt --all -- --check, cargo check --workspace und relevante cargo test-Befehle aus.
8. Behebe Fehler und Regressionen. Berichte ehrlich, welche Prüfungen tatsächlich ausgeführt wurden.
9. Aktualisiere Dokumentation und Beispiele bei Änderungen öffentlicher APIs.
10. Nenne am Ende geänderte Dateien, Designentscheidungen, Testergebnisse und den nächsten sinnvollen Schritt.
Qualitätsregeln
● Keine Breaking Changes ohne Begründung.
● Abhängigkeiten sparsam auswählen und Versionen konsistent verwalten.
● Öffentliche APIs dokumentieren und Fehler verständlich behandeln.
● Module über klar definierte Schnittstellen koppeln.
● Security- und Datenschutzanforderungen von Anfang an berücksichtigen.
● HTTP-Verhalten einschließlich HEAD/OPTIONS sowie deaktivierter TRACE/CONNECT-Routen testen.
● Für LLM-Funktionen Provider austauschbar halten, Secrets schützen, Kosten/Timeouts berücksichtigen und Tool-Aufrufe kontrollieren.
● Niemals behaupten, Tests seien erfolgreich, wenn sie nicht ausgeführt wurden.
● Bei Unklarheiten eine gezielte Rückfrage stellen; ansonsten eine konservative, erweiterbare Standardentscheidung treffen.
Empfohlene nächste Schritte
1. Repository und aktuelle Cargo-Dateien verifizieren.
2. Workspace und CLI kompilierbar machen.
3. oxbloom.toml bzw. ein Komponentenmanifest definieren.
4. oxbloom new so bauen, dass es ein minimales, kompilierbares Projekt erzeugt.
5. Gemeinsame Routenschnittstelle implementieren und Controller-Routen tatsächlich registrieren.
6. Danach oxbloom-ai als optionale Provider-abstrakte Komponente entwerfen und mit einem Mock-Provider testen.
7. Weitere Module schrittweise integrieren.
