import React, { useState, useCallback } from 'react';

interface LoginPageProps {
  onLogin: (token: string) => void;
}

export function LoginPage({ onLogin }: LoginPageProps) {
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const handleSubmit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      setError(null);

      if (!email.trim()) {
        setError('Email is required');
        return;
      }
      if (!password) {
        setError('Password is required');
        return;
      }

      setLoading(true);

      try {
        const res = await fetch('/api/v1/auth/login', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ email, password }),
        });

        if (!res.ok) {
          const body = await res.json().catch(() => null);
          throw new Error(
            body?.error?.message ?? `Login failed (${res.status})`
          );
        }

        const data = await res.json();
        const token: string = data.token ?? data.access_token;

        if (!token) {
          throw new Error('No token returned from server');
        }

        localStorage.setItem('orp_token', token);
        onLogin(token);
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Login failed');
      } finally {
        setLoading(false);
      }
    },
    [email, password, onLogin]
  );

  const handleSSOLogin = useCallback(() => {
    window.location.href = '/auth/login';
  }, []);

  return (
    <div className="h-screen w-screen flex items-center justify-center bg-gray-950">
      <div className="w-full max-w-sm mx-4">
        {/* Logo */}
        <div className="flex flex-col items-center mb-8">
          <div
            className="flex items-center justify-center w-12 h-12 rounded-none bg-blue-700 text-white text-lg font-bold tracking-tight mb-3"
            aria-hidden="true"
          >
            ORP
          </div>
          <h1 className="text-xl font-semibold text-gray-100">
            ORP Console
          </h1>
          <p className="text-xs text-gray-500 mt-1">
            Sign in to access ORP Console
          </p>
        </div>

        {/* Error message */}
        {error && (
          <div
            role="alert"
            className="mb-4 px-3 py-2 rounded-none bg-red-900/50 border border-red-800 text-red-300 text-xs"
          >
            {error}
          </div>
        )}

        {/* Login form */}
        <form onSubmit={handleSubmit} className="space-y-4" noValidate>
          <div>
            <label
              htmlFor="login-email"
              className="block text-xs font-medium text-gray-400 mb-1"
            >
              Email
            </label>
            <input
              id="login-email"
              type="email"
              autoComplete="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="you@example.com"
              className="w-full px-3 py-2 rounded-none bg-gray-900 border border-gray-800 text-gray-100 text-sm placeholder-gray-600 focus:outline-none focus:ring-2 focus:ring-blue-600 focus:border-transparent"
              disabled={loading}
              required
            />
          </div>

          <div>
            <label
              htmlFor="login-password"
              className="block text-xs font-medium text-gray-400 mb-1"
            >
              Password
            </label>
            <input
              id="login-password"
              type="password"
              autoComplete="current-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="••••••••"
              className="w-full px-3 py-2 rounded-none bg-gray-900 border border-gray-800 text-gray-100 text-sm placeholder-gray-600 focus:outline-none focus:ring-2 focus:ring-blue-600 focus:border-transparent"
              disabled={loading}
              required
            />
          </div>

          <button
            type="submit"
            disabled={loading}
            className="w-full py-2 rounded-none bg-blue-700 hover:bg-blue-600 text-white text-sm font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {loading ? 'Signing in…' : 'Sign in'}
          </button>
        </form>

        {/* Divider */}
        <div className="flex items-center gap-3 my-5">
          <div className="flex-1 h-px bg-gray-800" />
          <span className="text-[10px] text-gray-600 uppercase tracking-wider">
            or
          </span>
          <div className="flex-1 h-px bg-gray-800" />
        </div>

        {/* SSO */}
        <button
          type="button"
          onClick={handleSSOLogin}
          className="w-full py-2 rounded-none border border-gray-700 hover:border-gray-600 bg-gray-900 hover:bg-gray-800 text-gray-300 text-sm font-medium transition-colors"
        >
          Login with SSO
        </button>
      </div>
    </div>
  );
}
