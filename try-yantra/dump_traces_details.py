import sqlite3
import os

def dump():
    db_paths = [".yantra/traces.sqlite", "../.yantra/traces.sqlite"]
    for db_path in db_paths:
        if not os.path.exists(db_path):
            continue
        print(f"=== DB: {db_path} ===")
        conn = sqlite3.connect(db_path)
        try:
            sessions = conn.execute("SELECT DISTINCT session_id FROM spans").fetchall()
            print("Sessions in spans table:", sessions)
            count = conn.execute("SELECT COUNT(*) FROM spans").fetchone()[0]
            print("Total spans:", count)
            if count > 0:
                print("First 3 spans:")
                rows = conn.execute("SELECT * FROM spans LIMIT 3").fetchall()
                cols = [d[0] for d in conn.execute("SELECT * FROM spans LIMIT 1").description]
                for r in rows:
                    print(dict(zip(cols, r)))
        except Exception as e:
            print("Error:", e)

if __name__ == "__main__":
    dump()
