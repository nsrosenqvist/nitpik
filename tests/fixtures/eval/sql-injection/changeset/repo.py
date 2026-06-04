def find_user(cur, email):
    cur.execute(f"SELECT * FROM users WHERE email = '{email}'")
    return cur.fetchone()
