def load_count(path):
    try:
        with open(path) as f:
            return int(f.read())
    except Exception:
        return 0
