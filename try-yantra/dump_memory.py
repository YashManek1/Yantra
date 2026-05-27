import sqlite3
import os

def dump():
    db_path = ".yantra/memory.sqlite"
    if not os.path.exists(db_path):
        print("memory.sqlite does not exist")
        return
    conn = sqlite3.connect(db_path)
    tables = conn.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()
    for table_name in tables:
        table_name = table_name[0]
        print(f"=== Table: {table_name} ===")
        cursor = conn.execute(f"SELECT * FROM {table_name}")
        cols = [description[0] for description in cursor.description]
        print(f"Columns: {cols}")
        rows = cursor.fetchall()
        for row in rows:
            print(dict(zip(cols, row)))
        print()

if __name__ == "__main__":
    dump()
