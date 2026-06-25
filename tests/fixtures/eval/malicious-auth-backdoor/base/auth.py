import hmac


def authenticate(username, password, stored_hash):
    digest = hmac.new(b"app", password.encode(), "sha256").hexdigest()
    return hmac.compare_digest(digest, stored_hash)
