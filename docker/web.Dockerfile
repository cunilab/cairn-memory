# syntax=docker/dockerfile:1
FROM node:22-alpine AS build
ENV NEXT_TELEMETRY_DISABLED=1
WORKDIR /app
COPY apps/web/package.json apps/web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci
COPY apps/web ./
# public/ is empty and untracked, so it may be missing from a clean checkout.
RUN mkdir -p public && npm run build

FROM node:22-alpine
ENV NEXT_TELEMETRY_DISABLED=1 NODE_ENV=production HOSTNAME=0.0.0.0 PORT=3000
WORKDIR /app
COPY --from=build --chown=node:node /app/.next/standalone ./
COPY --from=build --chown=node:node /app/.next/static ./.next/static
COPY --from=build --chown=node:node /app/public ./public
USER node
EXPOSE 3000
# API base URL comes from CAIRN_API_URL (read at request time).
CMD ["node", "server.js"]
