import sqlite3
import json

conn = sqlite3.connect(".yantra/decisions.sqlite")
cursor = conn.execute("SELECT id, timestamp, action_type, reasoning, agent FROM decisions WHERE agent LIKE '%Coder%'")
for row in cursor.fetchall():
    print("---------------------------------------------")
    print(f"ID: {row[0]}")
    print(f"Timestamp: {row[1]}")
    print(f"Action Type: {row[2]}")
    print(f"Agent: {row[4]}")
    print(f"Reasoning:\n{row[3]}")
    print("---------------------------------------------")
