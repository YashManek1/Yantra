import sqlite3
import json
import os

def diagnose():
    db_paths = [".yantra/decisions.sqlite", "../.yantra/decisions.sqlite"]
    print("=== DIAGNOSING DECISIONS ===")
    for db_path in db_paths:
        if not os.path.exists(db_path):
            continue
        print(f"Reading from {db_path}...")
        conn_dec = sqlite3.connect(db_path)
        try:
            cursor = conn_dec.execute("SELECT id, timestamp, session_id, parent_decision_id, action_type, reasoning, agent FROM decisions WHERE session_id LIKE '%019e6af6%'")
            cols = [d[0] for d in cursor.description]
            rows = cursor.fetchall()
            print(f"Found {len(rows)} decisions:")
            for row in rows:
                print("--- Decision ---")
                for col, val in zip(cols, row):
                    print(f"{col}: {val}")
        except Exception as e:
            print("Error reading decisions:", e)

    trace_paths = [".yantra/traces.sqlite", "../.yantra/traces.sqlite"]
    print("\n=== DIAGNOSING SPANS ===")
    for trace_path in trace_paths:
        if not os.path.exists(trace_path):
            continue
        print(f"Reading from {trace_path}...")
        conn_trace = sqlite3.connect(trace_path)
        try:
            cursor = conn_trace.execute("SELECT span_id, parent_id, session_id, task_id, agent, model, outcome, error_kind, error_message FROM spans WHERE session_id LIKE '%019e6af6%'")
            cols = [d[0] for d in cursor.description]
            rows = cursor.fetchall()
            print(f"Found {len(rows)} spans:")
            for row in rows:
                span_dict = dict(zip(cols, row))
                print("--- Span ---")
                for col, val in zip(cols, row):
                    print(f"{col}: {val}")
        except Exception as e:
            print("Error reading spans:", e)

if __name__ == "__main__":
    diagnose()
