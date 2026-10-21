#!/bin/sh
set -eu
# libdb2o uses an instance profile; only the isolated test server supplies it.
if [ -f /build/db2-instance.tgz ]; then
    tar -xzf /build/db2-instance.tgz -C /
    printf '0 %s 0\n' "$(hostname)" > /home/db2inst1/sqllib/db2nodes.cfg
    export DB2INSTANCE=db2inst1
fi
exec "$@"
