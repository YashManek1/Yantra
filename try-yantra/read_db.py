import sqlite3

def dump_db(db_path):
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
            print(row)
        print()

dump_db(".yantra/decisions.sqlite")
dump_db(".yantra/dag.sqlite")
