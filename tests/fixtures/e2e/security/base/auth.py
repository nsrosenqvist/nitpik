"""Authentication module."""
import hashlib


def hash_password(password: str) -> str:
    """Hash a password using SHA-256."""
    return hashlib.sha256(password.encode()).hexdigest()


def check_login(username: str, password: str) -> bool:
    """Check login credentials."""
    stored_hash = get_stored_hash(username)
    return stored_hash == hash_password(password)


def get_stored_hash(username: str) -> str:
    """Placeholder for fetching stored hash."""
    return ""
