# Changes to shared files

A task changes shared files when it touches what every other task also depends on. It cannot run beside another
task in the same folder: it runs alone.

Dependencies: add a package, install a library, remove a dependency, upgrade a dependency, bump React, bump Next.js,
update package.json, update the lockfile, bun.lock, package-lock.json, yarn.lock, pnpm-lock.yaml, Cargo.toml and
Cargo.lock, requirements.txt, pyproject.toml, poetry.lock, go.mod and go.sum, Gemfile.lock, composer.lock.

Database structure: add a migration, write a migration, change the schema, add a column, add a table, rename a column,
add an index, change a foreign key, seed data, Prisma schema, Drizzle schema, an ORM model change.

Generated code: regenerate types, regenerate the API client, run codegen, update the GraphQL schema, protobuf, OpenAPI
spec, update snapshots across the project.

Build and tooling: change the build configuration, webpack, Vite, Turbopack, tsconfig, Babel, ESLint config, Prettier
config, formatter settings, the CI pipeline, GitHub Actions, Dockerfile, docker-compose, Makefile, environment
templates.

Central files: the global stylesheet, the theme, design tokens, the routes table, the app layout, the translations
catalogue, the main config file, the dependency injection container, the root module.

Project-wide edits: rename across the whole project, move many files, reorganise folders, reformat the whole codebase,
search and replace everywhere, change an import path used everywhere.
