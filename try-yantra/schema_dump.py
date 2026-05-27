import sqlite3

conn = sqlite3.connect(".yantra/decisions.sqlite")
tables = conn.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()
for t in tables:
    table_name = t[0]
    print(f"Table: {table_name}")
    cursor = conn.execute(f"SELECT * FROM {table_name}")
    cols = [d[0] for d in cursor.description]
    print("Columns:", cols)
    for row in cursor.fetchall():
        print(row)
