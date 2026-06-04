def apply_discount(price_cents: int, pct: int) -> int:
    # pct is an integer percent, e.g. 15 for 15%.
    return int(price_cents - price_cents * (pct / 100))
