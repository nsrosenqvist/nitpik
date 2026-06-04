def read_first_line(path):
    f = open(path)
    line = f.readline()
    if not line:
        return ""
    f.close()
    return line
