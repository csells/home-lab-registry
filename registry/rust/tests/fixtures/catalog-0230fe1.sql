BEGIN TRANSACTION;
CREATE TABLE health (id TEXT PRIMARY KEY, status TEXT NOT NULL, evidence TEXT NOT NULL, checked REAL NOT NULL, expires REAL NOT NULL);
INSERT INTO "health" VALUES('service-test','healthy','fixture check',1795418420.888706,1795418720.888706);
CREATE TABLE history (seq INTEGER PRIMARY KEY, id TEXT NOT NULL, revision INTEGER NOT NULL, document TEXT NOT NULL, actor TEXT NOT NULL, updated REAL NOT NULL);
INSERT INTO "history" VALUES(1,'service-test',1,'{"capabilities": ["test"], "dependencies": [], "endpoints": {}, "host": "test", "lifecycle": "active", "name": "Original", "owner": "fixtures", "resources": {"runtime_binding": {"instance": "service", "kind": "native", "provider": "test"}}, "runbook": "test"}','writer',1795418420.880507);
CREATE TABLE presence (id TEXT PRIMARY KEY, address TEXT NOT NULL, interface TEXT NOT NULL, checked REAL NOT NULL, expires REAL NOT NULL);
CREATE TABLE runtime_observation (
                id TEXT PRIMARY KEY, state TEXT NOT NULL, checked REAL NOT NULL, expires REAL NOT NULL);
INSERT INTO "runtime_observation" VALUES('service-test','present',1795418420.8936441,1795418540.8936441);
CREATE TABLE services (id TEXT PRIMARY KEY, revision INTEGER NOT NULL, document TEXT NOT NULL, updated REAL NOT NULL);
INSERT INTO "services" VALUES('service-test',1,'{"capabilities": ["test"], "dependencies": [], "endpoints": {}, "host": "test", "lifecycle": "active", "name": "Original", "owner": "fixtures", "resources": {"runtime_binding": {"instance": "service", "kind": "native", "provider": "test"}}, "runbook": "test"}',1795418420.880507);
COMMIT;
