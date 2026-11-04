FROM node:22-alpine AS build
WORKDIR /src
RUN corepack enable
COPY package.json pnpm-workspace.yaml pnpm-lock.yaml ./
COPY apps/web/package.json apps/web/package.json
RUN pnpm install --frozen-lockfile
COPY apps/web apps/web
COPY fixtures fixtures
COPY plugins plugins
# The web app reads the standard record field catalogue (ADR 0014).
COPY docs/architecture/standard-record-fields.json docs/architecture/standard-record-fields.json
RUN pnpm --filter web build
FROM nginxinc/nginx-unprivileged:1.27-alpine
# Same-origin API proxy and single-page-app fallback; see the template.
ENV API_UPSTREAM=api:8080 \
    NGINX_RESOLVER=127.0.0.11
COPY infra/web/default.conf.template /etc/nginx/templates/default.conf.template
COPY --from=build /src/apps/web/dist /usr/share/nginx/html
