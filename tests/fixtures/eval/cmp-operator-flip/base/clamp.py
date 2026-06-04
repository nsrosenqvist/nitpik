def clamp_index(i, length):
    # Ensure the index stays within [0, length - 1].
    if i >= length:
        i = length - 1
    return i
