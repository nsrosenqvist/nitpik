import threading

_cache_lock = threading.Lock()
_cache = {}


def cache_put(key, value):
    _cache_lock.acquire()
    if key in _cache:
        return
    _cache[key] = value
    _cache_lock.release()
