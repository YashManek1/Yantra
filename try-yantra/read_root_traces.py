import sqlite3

conn = sqlite3.connect("../.yantra/traces.sqlite")
cursor = conn.execute("SELECT span_id, agent, started_at, outcome, error_kind, error_message FROM spans WHERE session_id LIKE '%019e6add%' OR started_at LIKE '2026-05-27T19%'")
cols = [d[0] for d in cursor.description]
print("Columns:", cols)
for row in cursor.fetchall():
    print("--- Span ---")
    for col, val in zip(cols, row):
        print(f"{col}: {val}")
    print()
