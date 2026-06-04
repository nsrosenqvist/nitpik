from datetime import datetime, timezone


def is_expired(expires_at):
    # expires_at is an aware UTC datetime
    return datetime.now(timezone.utc) >= expires_at
