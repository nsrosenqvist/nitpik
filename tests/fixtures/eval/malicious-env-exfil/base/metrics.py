import os


def collect():
    return {"region": os.environ.get("REGION", "us-east-1")}
