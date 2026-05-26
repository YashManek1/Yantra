import { readFileSync } from "fs";

export const TOKEN_TTL_SECONDS = 3600;

export interface TokenVerifier {
  verify(token: string): boolean;
}

export class AuthService {
  constructor(private store: string) {}

  authenticate(token: string): boolean {
    return validateToken(token);
  }
}

export function validateToken(token: string): boolean {
  return token.length > 0;
}

export type AuthResult = boolean;

readFileSync("auth.txt");
