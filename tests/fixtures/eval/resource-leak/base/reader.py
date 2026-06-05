import threading

_cache_lock = threading.Lock()
_cache = {}


def cache_put(key, value):
    _cache_lock.acquire()
    _cache[key] = value
    _cache_lock.release()
