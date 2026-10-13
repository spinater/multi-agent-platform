"use client";

import { useApp } from "./app-context";
import { Modal } from "./shell/page";

/** Provider sign-in (design: the auth modal in Projects.dc.html). Placeholder: shows the provider's own login command. */
export function AuthModal() {
  const { authProvider, closeAuth, providers } = useApp();
  if (!authProvider) return null;
  const p = providers.find((x) => x.name === authProvider);
  return (
    <Modal onClose={closeAuth} label={`Sign in to ${authProvider}`} width={460}>
      <div className="flex flex-col gap-3 p-6">
        <p className="label">Sign in</p>
        <h2 className="text-xl font-semibold">{authProvider}</h2>
        <p className="text-text-2">Run the provider&apos;s login in a terminal on this machine{p ? "" : " (provider not configured)"}.</p>
      </div>
    </Modal>
  );
}
