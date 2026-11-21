import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  /* config options here */
  output: "standalone",
  cacheComponents: true,
  partialPrefetching: true,
  // The old per-feature pages now live in tabs of Activity and Settings.
  async redirects() {
    return [
      { source: "/devices", destination: "/settings?tab=devices", permanent: false },
      { source: "/accounts", destination: "/settings?tab=accounts", permanent: false },
      { source: "/jobs", destination: "/activity?tab=jobs", permanent: false },
    ];
  },
  turbopack: {
    rules: {
      "*.css": {
        loaders: ["@tailwindcss/turbopack"],
        as: "*.css",
      },
    },
  },
};

export default nextConfig;
