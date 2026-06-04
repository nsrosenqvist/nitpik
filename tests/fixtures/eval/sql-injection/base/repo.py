def find_user(cur, email):
    cur.execute("SELECT * FROM users WHERE email = %s", (email,))
    return cur.fetchone()
