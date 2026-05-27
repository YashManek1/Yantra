import sqlite3

conn = sqlite3.connect(".yantra/decisions.sqlite")
row = conn.execute("SELECT alternatives_json FROM decisions WHERE id = '019e6ade-0117-7c10-b675-f0208ef13e66'").fetchone()
if row:
    print(row[0])
