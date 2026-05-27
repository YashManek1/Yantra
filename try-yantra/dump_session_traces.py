import sqlite3

def dump():
    conn = sqlite3.connect(".yantra/traces.sqlite")
    cursor = conn.execute("SELECT span_id, parent_id, session_id, task_id, agent, model, outcome, error_kind, error_message FROM spans WHERE session_id = '019e6af1-7788-7950-b31c-3fafb9704dff'")
    cols = [d[0] for d in cursor.description]
    rows = cursor.fetchall()
    print(f"Found {len(rows)} spans:")
    for row in rows:
        print("--- Span ---")
        for col, val in zip(cols, row):
            print(f"{col}: {val}")

if __name__ == "__main__":
    dump()
