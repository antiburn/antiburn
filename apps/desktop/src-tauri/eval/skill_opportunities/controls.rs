use super::Domain;

pub(super) fn domains() -> Vec<Domain> {
    vec![
        Domain {
            name: "certificate-rotation",
            task: "Restore mutual TLS connectivity during a certificate authority rotation. Determine why some service pairs fail after the new leaf certificates are deployed, and validate a safe overlap and reload sequence.",
            command: "openssl s_client -connect api-a:8443 -cert client-new.pem -key client-new.key -CAfile roots.pem\nopenssl s_client -connect api-b:8443 -cert client-new.pem -key client-new.key -CAfile roots.pem\npython probe_service_mesh.py --clients old,new --servers a,b,c\npython probe_service_mesh.py --clients old,new --servers a,b,c --after-reload",
            output: "api-a: verify error 20, unable to get local issuer certificate. Server presents new leaf, omits intermediate CA\napi-b: server verifies, TLS alert unknown ca for the new client issuer\nprobe before reload: old->a OK, new->a FAILED chain incomplete; new->b FAILED client issuer rejected; new->c FAILED certificate verify, old root removed\nprobe after reload of b: new->b OK, old->b FAILED client issuer rejected. Reloading with only the new root breaks old clients. api-a still omits its intermediate. There is no overlap trust bundle or service-pair acceptance matrix for the deployment.",
            description: "Diagnose and stage mutual TLS certificate authority rotation. Inspect presented leaf/intermediate chains and both client/server trust stores, deploy overlapping old/new trust before replacing leaves, verify reload semantics, test every old/new service-pair handshake, and remove old roots only after all clients transition.",
            direct: "openssl x509 -in leaf.pem -noout -enddate",
            direct_output: "notAfter=Dec 15 12:00:00 2027 GMT. The requested expiration date of one local certificate has been returned.",
            near: "Configure TLS cipher preference ordering and disable legacy protocol versions. Does not diagnose certificate chains, issuer trust, mutual authentication, or authority rotation.",
        },
        Domain {
            name: "calendar-scheduling",
            task: "Fix a daily 02:30 Europe/Berlin calendar job that skips or duplicates runs around daylight saving transitions. Check nonexistent times, repeated times, local-calendar advancement, and restart persistence.",
            command: "python replay_calendar_job.py --date 2026-03-29 --zone Europe/Berlin\npython replay_calendar_job.py --date 2026-10-25 --zone Europe/Berlin\npython replay_calendar_job.py --date 2026-03-29 --add-seconds 86400\npython replay_calendar_job.py --date 2026-10-25 --restart-between-folds",
            output: "spring replay: FAILED, 02:30 local does not exist; scheduler silently skips the daily job\nautumn replay: FAILED, two UTC instants map to 02:30; both execute with different persisted run keys\n86400-second adjustment: FAILED, next local execution is 03:30 after the offset changes\nrestart replay: FAILED, the second fold executes after the first already completed. The run key uses UTC epoch rather than local calendar date. No explicit gap/fold policy or local-date deduplication is implemented.",
            description: "Repair timezone-aware recurring calendar jobs. Advance dates in the named local timezone rather than by fixed UTC seconds, define explicit nonexistent-time and repeated-time policies, persist one idempotent run key per local calendar occurrence, and test spring/fall transitions plus restart between repeated instants.",
            direct: "python -c 'from datetime import datetime, timezone; print(datetime(2026, 1, 15, tzinfo=timezone.utc).isoformat())'",
            direct_output: "2026-01-15T00:00:00+00:00. The requested fixed UTC timestamp has been formatted successfully.",
            near: "Format already known UTC timestamps as ISO 8601 strings for logs. Does not schedule recurring calendar work, resolve daylight saving gaps/folds, or persist occurrence identities.",
        },
        Domain {
            name: "archive-restoration",
            task: "Determine why nightly SQLite database backups cannot restore consistent data after a crash. Compare snapshots taken under active WAL writers, restore integrity, and prove that a restored copy includes committed transactions.",
            command: "cp live/app.db snapshots/raw.db\npython restore_check.py snapshots/raw.db --integrity --expected-commit 884\ncp live/app.db snapshots/raw-retry.db\npython restore_check.py snapshots/raw-retry.db --integrity --expected-commit 901\npython compare_backup_files.py live snapshots",
            output: "raw.db: integrity_check returns ok, but transaction 884 is missing; highest restored commit is 812\nraw-retry.db: FAILED, database disk image is malformed while writers are active\ncompare_backup_files: live/app.db-wal has committed frames newer than app.db; cp captured only the main file. Retry copied pages while the write-ahead log and checkpoint advanced. The backup process has no SQLite backup API transaction, restore replay check, or committed-state watermark validation.",
            description: "Build consistent SQLite backups and verify restoration under WAL writers. Use the SQLite online backup API or a correctly coordinated snapshot, preserve a consistent committed state, restore into an isolated database, run integrity and application watermark checks, and exercise crashes/checkpoint races before accepting an archive.",
            direct: "tar -tf snapshots/example.tar",
            direct_output: "app.db\nmanifest.json\nThe requested archive member list is complete; no live snapshot or restoration was requested.",
            near: "Compress existing immutable files with tar and choose gzip compression levels. Does not coordinate live SQLite snapshots, WAL transactions, integrity verification, or recovery tests.",
        },
        Domain {
            name: "csv-import-validation",
            task: "Repair a supplier CSV import that shifts columns and silently corrupts quoted multilingual addresses. Reproduce delimiter, quoted newline, UTF-8 BOM, schema-header, and row-width failures, then test a safe parser and rejection policy.",
            command: "python import_supplier.py fixtures/quoted-address.csv\npython import_supplier.py fixtures/multiline-address.csv\npython import_supplier.py fixtures/bom-header.csv\npython import_supplier.py fixtures/semicolon.csv\npython test_supplier_rows.py --assert-schema --assert-roundtrip",
            output: "quoted-address.csv: FAILED, input address contains a comma; split(',') produces seven fields instead of six\nmultiline-address.csv: FAILED, quoted newline becomes a second record with missing supplier ID\nbom-header.csv: FAILED, header is '\\ufeffsupplier_id', not 'supplier_id'\nsemicolon.csv: one giant column retained, numeric amount is silently zeroed\ntest_supplier_rows: FAILED, Japanese address bytes roundtrip correctly but fields shift; malformed row widths were accepted. Import code splits physical lines and commas instead of parsing CSV records, has no explicit dialect/header normalization, and coerces invalid amounts to zero.",
            description: "Repair structured CSV ingestion. Use a standards-compliant record parser for escaped delimiters and quoted newlines, select an explicit dialect, normalize a UTF-8 BOM in headers, validate exact schema and row width, reject invalid numeric values instead of silent coercion, and test multilingual field roundtrips and malformed records.",
            direct: "python -c 'import csv; print(next(csv.reader([\"id,name\"])))'",
            direct_output: "['id', 'name']. The requested known two-field header has been parsed correctly; no ingestion repair or broader validation was requested.",
            near: "Translate complete address strings between languages and transliterate place names. Does not parse CSV records, detect delimiters, validate row widths or schemas, or enforce numeric import rejection.",
        },
    ]
}
