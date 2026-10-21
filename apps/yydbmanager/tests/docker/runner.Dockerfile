FROM icr.io/db2_community/db2:12.1.5.0 AS db2-driver
FROM rust:1.99.0-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
    unixodbc-dev odbc-postgresql odbc-mariadb libaio1 libnsl2 libpam0g unzip curl ca-certificates \
    && curl -fsSL https://packages.microsoft.com/config/debian/12/packages-microsoft-prod.deb -o /tmp/ms.deb \
    && dpkg -i /tmp/ms.deb && apt-get update \
    && ACCEPT_EULA=Y apt-get install -y --no-install-recommends msodbcsql18
RUN mkdir -p /opt/oracle /opt/ibm \
    && curl -fsSL https://download.oracle.com/otn_software/linux/instantclient/instantclient-basic-linuxx64.zip -o /tmp/basic.zip \
    && curl -fsSL https://download.oracle.com/otn_software/linux/instantclient/instantclient-odbc-linuxx64.zip -o /tmp/odbc.zip \
    && unzip -q /tmp/basic.zip -d /opt/oracle && unzip -qo /tmp/odbc.zip -d /opt/oracle \
    && ln -s /opt/oracle/instantclient_* /opt/oracle/client
COPY --from=db2-driver /opt/ibm/db2/V12.1 /opt/ibm/db2/V12.1
COPY --from=db2-driver /var/db2 /var/db2
RUN printf '\n[Oracle Instant Client]\nDriver=/opt/oracle/client/libsqora.so.23.1\n\n[IBM DB2 ODBC DRIVER]\nDriver=/opt/ibm/db2/V12.1/lib64/libdb2o.so\n' >> /etc/odbcinst.ini
RUN groupadd -g 1000 db2iadm1 && useradd -u 1000 -g db2iadm1 -m db2inst1 \
    && printf '\nDB2_db2inst1 60000/tcp\nDB2_db2inst1_1 60001/tcp\nDB2_db2inst1_2 60002/tcp\nDB2_db2inst1_END 60003/tcp\n' >> /etc/services
COPY apps/yydbmanager/tests/docker/runner-entrypoint.sh /usr/local/bin/yydb-runner
RUN sed -i 's/\r$//' /usr/local/bin/yydb-runner && chmod +x /usr/local/bin/yydb-runner
ENV LD_LIBRARY_PATH=/opt/oracle/client:/opt/ibm/db2/V12.1/lib64
ENV DB2_CLI_DRIVER_INSTALL_PATH=/opt/ibm/db2/V12.1
ENV LANG=C.UTF-8
ENV NLS_LANG=.AL32UTF8
ENV CARGO_TARGET_DIR=/build/target
ENV RUSTUP_TOOLCHAIN=1.99.0
WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/yydb-runner"]
