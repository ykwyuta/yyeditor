FROM icr.io/db2_community/db2:12.1.5.0
COPY init/db2.sql /demo/init.sql
COPY sql /demo/sql
COPY --chmod=755 docker/db2-init.sh /var/custom/10-demo.sh
COPY --chmod=755 docker/db2-health.sh /demo/db2-health.sh
