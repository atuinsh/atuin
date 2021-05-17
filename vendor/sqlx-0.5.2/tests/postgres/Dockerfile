ARG VERSION
FROM postgres:${VERSION}-alpine

# Copy SSL certificate (and key)
COPY certs/server.crt /var/lib/postgresql/server.crt
COPY keys/server.key /var/lib/postgresql/server.key

# Fix permissions
RUN chown 70:70 /var/lib/postgresql/server.crt /var/lib/postgresql/server.key
RUN chmod 0600 /var/lib/postgresql/server.crt /var/lib/postgresql/server.key
