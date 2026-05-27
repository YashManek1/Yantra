import sqlite3

def dump_traces(db_path):
    print(f"=== {db_path} ===")
    conn = sqlite3.connect(db_path)
    tables = conn.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()
    for table_name in tables:
        table_name = table_name[0]
        print(f"Table: {table_name}")
        cursor = conn.execute(f"SELECT * FROM {table_name}")
        cols = [description[0] for description in cursor.description]
        print(f"Columns: {cols}")
        rows = cursor.fetchall()
        for row in rows:
            # truncate long values for readability
            short_row = [str(val)[:200] for val in row]
            print(short_row)
        print()

dump_traces(".yantra/traces.sqlite")
