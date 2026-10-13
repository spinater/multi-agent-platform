import type { NextConfig } from "next";

// Production: a static export (`out/`) that the Rust server serves next to /api, one process and one port.
// `next dev`: static exports cannot rewrite, so the export is switched off and /api is proxied to the backend.
// The dashboard's WebSocket goes straight to the backend in dev (see lib/api.ts).
const backend = process.env.MAP_BACKEND ?? "http://127.0.0.1:4400";
const isDev = process.env.NODE_ENV === "development";

const nextConfig: NextConfig = {
  turbopack: {
    rules: {
      "*.css": {
        loaders: ["@tailwindcss/turbopack"],
        as: "*.css",
      },
    },
  },
  ...(isDev
    ? {
        async rewrites() {
          return [{ source: "/api/:path*", destination: `${backend}/api/:path*` }];
        },
      }
    : { output: "export" as const }),
};

export default nextConfig;
