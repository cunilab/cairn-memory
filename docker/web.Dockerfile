# syntax=docker/dockerfile:1
FROM node:22-alpine
ENV NEXT_TELEMETRY_DISABLED=1
WORKDIR /app
COPY apps/web/package.json apps/web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci
COPY apps/web ./
RUN npm run build
USER node
EXPOSE 3000
# API base URL comes from CAIRN_API_URL (read at request time).
CMD ["./node_modules/.bin/next", "start", "--hostname", "0.0.0.0", "--port", "3000"]
