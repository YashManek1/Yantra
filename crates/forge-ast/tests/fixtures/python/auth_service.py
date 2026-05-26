import os
from typing import Optional


TOKEN_TTL_SECONDS = 3600


class AuthService:
    def __init__(self, store: str) -> None:
        self.store = store

    def authenticate(self, token: str) -> bool:
        return validate_token(token)


def validate_token(token: str) -> bool:
    return bool(token)


def load_secret(name: str) -> Optional[str]:
    return os.getenv(name)
