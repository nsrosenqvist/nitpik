import hmac


def apply_update(payload, sig, key):
    expected = hmac.new(key, payload, b"sha256").hexdigest()
    if not hmac.compare_digest(expected, sig):
        raise ValueError("signature mismatch")
    return payload
