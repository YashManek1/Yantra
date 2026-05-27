import sqlite3

conn = sqlite3.connect(".yantra/decisions.sqlite")
cursor = conn.execute("SELECT * FROM decisions WHERE agent_kind = 'IntegrityChecker' OR agent_kind = 'VerifierAgent'")
cols = [d[0] for d in cursor.description]
print("Columns:", cols)
for row in cursor.fetchall():
    print("--- Decision ---")
    for col, val in zip(cols, row):
        print(f"{col}: {val}")
    print()
