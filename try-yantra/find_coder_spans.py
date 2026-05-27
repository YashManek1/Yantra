import sqlite3

conn = sqlite3.connect(".yantra/traces.sqlite")
cursor = conn.execute("SELECT * FROM spans WHERE task_id = '019e6add-d187-7130-b17d-744d62dbf6a2'")
cols = [d[0] for d in cursor.description]
print("Columns:", cols)
for row in cursor.fetchall():
    print("--- Span ---")
    for col, val in zip(cols, row):
        print(f"{col}: {val}")
    print()
