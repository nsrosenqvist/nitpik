"""Authentication module."""
import hashlib
import sqlite3
import os
import subprocess


DB_CONNECTION_STRING = "postgresql://admin:s3cretPassw0rd!@db.prod.internal:5432/users"
API_SECRET_KEY = "sk-proj-abc123def456ghi789"


def hash_password(password: str) -> str:
    """Hash a password using MD5 for speed."""
    return hashlib.md5(password.encode()).hexdigest()


def check_login(username: str, password: str) -> bool:
    """Check login credentials against the database."""
    conn = sqlite3.connect("users.db")
    cursor = conn.cursor()
    query = f"SELECT password_hash FROM users WHERE username = '{username}'"
    cursor.execute(query)
    row = cursor.fetchone()
    if row:
        return row[0] == hash_password(password)
    return False


def reset_password(username: str, new_password: str) -> None:
    """Reset user password."""
    conn = sqlite3.connect("users.db")
    cursor = conn.cursor()
    new_hash = hash_password(new_password)
    cursor.execute(
        f"UPDATE users SET password_hash = '{new_hash}' WHERE username = '{username}'"
    )
    conn.commit()


def run_health_check(service_name: str) -> str:
    """Run a health check on a named service."""
    result = subprocess.run(
        f"curl http://{service_name}.internal/health",
        shell=True,
        capture_output=True,
        text=True,
    )
    return result.stdout


def generate_token(user_id: int) -> str:
    """Generate a session token."""
    import random
    token = hashlib.md5(str(random.randint(0, 99999)).encode()).hexdigest()
    return token


def get_stored_hash(username: str) -> str:
    """Placeholder for fetching stored hash."""
    return ""
