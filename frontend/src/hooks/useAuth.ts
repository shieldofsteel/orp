/**
 * useAuth — JWT-based authentication hook for ORP Console.
 *
 * Provides:
 *   - login(email, password)  — local credentials auth
 *   - loginWithSSO()          — OIDC redirect
 *   - logout()                — clear token + redirect
 *   - getToken()              — raw JWT string | null
 *   - isAuthenticated         — boolean
 *   - user                    — decoded JWT claims | null
 */

import { useState, useEffect, useCallback } from 'react';

// ── Types ─────────────────────────────────────────────────────────────────────

export interface JwtClaims {
  sub: string;           // subject (user id)
  email?: string;
  name?: string;
  org_id?: string;
  permissions?: string[];
  roles?: string[];
  exp?: number;          // expiry unix timestamp
  iat?: number;          // issued-at unix timestamp
}

export interface AuthUser {
  id: string;
  email: string;
  name: string;
  orgId: string;
  permissions: string[];
  roles: string[];
  expiresAt: Date | null;
}

export interface UseAuthReturn {
  isAuthenticated: boolean;
  isLoading: boolean;
  user: AuthUser | null;
  error: string | null;
  login: (email: string, password: string) => Promise<void>;
  loginWithSSO: () => void;
  logout: () => void;
  getToken: () => string | null;
}

// ── Constants ─────────────────────────────────────────────────────────────────

const TOKEN_KEY = 'orp_auth_token';
const API_BASE = import.meta.env.VITE_API_URL ?? '/api/v1';
const SSO_URL = import.meta.env.VITE_SSO_URL ?? '/auth/oidc/start';

// ── Helpers ───────────────────────────────────────────────────────────────────

/**
 * Decode a JWT payload without verifying the signature.
 * Verification is the server's responsibility; the client just reads claims.
 */
function decodeJwt(token: string): JwtClaims | null {
  try {
    const parts = token.split('.');
    if (parts.length !== 3) return null;

    // Base64url → Base64 → JSON
    const payload = parts[1].replace(/-/g, '+').replace(/_/g, '/');
    const padded = payload + '='.repeat((4 - (payload.length % 4)) % 4);
    const decoded = atob(padded);
    return JSON.parse(decoded) as JwtClaims;
  } catch {
    return null;
  }
}

function claimsToUser(claims: JwtClaims): AuthUser {
  return {
    id: claims.sub,
    email: claims.email ?? '',
    name: claims.name ?? claims.email ?? claims.sub,
    orgId: claims.org_id ?? '',
    permissions: claims.permissions ?? [],
    roles: claims.roles ?? [],
    expiresAt: claims.exp ? new Date(claims.exp * 1000) : null,
  };
}

function isTokenExpired(claims: JwtClaims): boolean {
  if (!claims.exp) return false; // no expiry set → treat as valid
  return Date.now() >= claims.exp * 1000;
}

// ── Hook ──────────────────────────────────────────────────────────────────────

export function useAuth(): UseAuthReturn {
  const [token, setToken] = useState<string | null>(() =>
    localStorage.getItem(TOKEN_KEY),
  );
  const [user, setUser] = useState<AuthUser | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Re-derive user whenever token changes.
  useEffect(() => {
    if (!token) {
      setUser(null);
      return;
    }
    const claims = decodeJwt(token);
    if (!claims || isTokenExpired(claims)) {
      // Token is invalid or expired — clear it silently.
      localStorage.removeItem(TOKEN_KEY);
      setToken(null);
      setUser(null);
      return;
    }
    setUser(claimsToUser(claims));
  }, [token]);

  const getToken = useCallback((): string | null => {
    return localStorage.getItem(TOKEN_KEY);
  }, []);

  const login = useCallback(async (email: string, password: string) => {
    setIsLoading(true);
    setError(null);
    try {
      const response = await fetch(`${API_BASE}/auth/login`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email, password }),
      });

      if (!response.ok) {
        const body = await response.json().catch(() => ({}));
        const message =
          body?.error?.message ??
          (response.status === 401
            ? 'Invalid email or password.'
            : `Login failed (HTTP ${response.status}).`);
        throw new Error(message);
      }

      const data = await response.json();
      const jwt: string = data?.token ?? data?.access_token ?? data?.jwt;
      if (!jwt) throw new Error('Server did not return a token.');

      localStorage.setItem(TOKEN_KEY, jwt);
      setToken(jwt);
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Login failed.';
      setError(message);
      throw err; // re-throw so LoginPage can react
    } finally {
      setIsLoading(false);
    }
  }, []);

  const loginWithSSO = useCallback(() => {
    // Redirect to OIDC provider; after auth it redirects back with a token in
    // the URL hash / query param which the app picks up on load.
    const returnUrl = encodeURIComponent(window.location.origin + '/');
    window.location.href = `${SSO_URL}?return_to=${returnUrl}`;
  }, []);

  const logout = useCallback(() => {
    localStorage.removeItem(TOKEN_KEY);
    setToken(null);
    setUser(null);
    setError(null);
  }, []);

  const isAuthenticated = !!user;

  return {
    isAuthenticated,
    isLoading,
    user,
    error,
    login,
    loginWithSSO,
    logout,
    getToken,
  };
}
