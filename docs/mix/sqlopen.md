# sqlopen — SQLite files: sqlopen, sqlexec, sqlclose

Mix has an embedded SQLite client (the `sqlite` feature; the `mix` binary
turns it on), so a script can create, query and update a database file
without the `sqlite3` CLI. Handles are plain numbers. These builtins are
`FsWrite`: they are arbitrary filesystem authority. A host that hands a
script one store should use the scoped `Db` seam (`db_query`) instead
(`mix man capabilities`).

```
sqlopen(path[, mode])      open a database             -> numeric handle
sqlexec(h, sql[, params])  run one statement           -> rows or {affected}
sqlclose(h)                close the handle (an unknown handle raises)
```

## Modes

| `mode` | Opens | A missing file |
|---|---|---|
| omitted, or `"ro"` | read-only | raises `SQL_NO_DATABASE`; nothing is created |
| `"rwc"` | read-write, WAL journal, 5 s busy timeout | is created |
| `"rw"` | the same as `"rwc"` | is created |

The default is **read-only**, so a script that only reads a database (a
secrets store, another program's state) cannot change it, or its journal
mode, by accident. A write through a read-only handle raises. To create a
database, say so with `"rwc"`. Any other mode, including `nil`, `"RW"` and
`""`, raises `OPTION_INVALID`; before 0.117.0 an unknown mode silently
opened read-only, which is why `sqlopen(path, "rwc")` used to fail on a new
file with only `unable to open database file`.

Opening read-write sets `PRAGMA journal_mode=WAL`, which is persistent: the
file stays in WAL mode and gets `-wal` and `-shm` siblings while it is open.
Open another program's database read-only unless the script owns it.

`":memory:"` is a private in-memory database that lives as long as the
handle. Open it with `"rw"`, since a read-only one can never hold a table.

```mix
$path = "/tmp/sqlopen-demo.db"
$db = sqlopen($path, "rwc")
sqlexec($db, "CREATE TABLE IF NOT EXISTS seen (id TEXT PRIMARY KEY, at INTEGER)")
sqlexec($db, "INSERT OR IGNORE INTO seen VALUES (?, ?)", ["msg-1", 1760000000])
sqlclose($db)

$ro = sqlopen($path)
print(json_encode(sqlexec($ro, "SELECT id FROM seen")))
sqlclose($ro)
```

```text
[{"id":"msg-1"}]
```

## Results and parameters

`sqlexec` returns a **list of row maps** for anything that produces columns
(`SELECT`, row-returning `PRAGMA`s) and `{affected: n}` for a write. SQLite
decides which (`sqlite3_stmt_readonly`), so `REPLACE` and `WITH … INSERT`
take the write path. Column values map NULL to `nil`, INTEGER and REAL to a
number, and TEXT to a string. A BLOB comes back as the placeholder string
`<blob N bytes>`, not as bytes.

`params` binds `?` placeholders: a list binds positionally, and a single
non-list value is one bind. Binds are typed: `nil` is NULL, a bool is
INTEGER 0/1, a whole number is INTEGER, a fractional one is REAL, a string
is TEXT and bytes are a BLOB. A list or map param raises. Always bind data;
never splice values into the SQL string.

## Errors

| Code | When |
|---|---|
| `SQL_NO_DATABASE` | a read-only open of a path that does not exist |
| `OPTION_INVALID` | a mode other than `"ro"`, `"rw"` or `"rwc"` |
| (runtime error) | SQLite refused the open or the statement; the message is SQLite's |

## See also

- [io](io.md): the rest of the filesystem builtins.
- [data](data.md): how Mix values cross serialization boundaries.
