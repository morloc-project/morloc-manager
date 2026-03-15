ARG CONTAINER_BASE
FROM ${CONTAINER_BASE}
LABEL morloc.environment="sample"
ENV MORLOC_ENV_NAME="sample"

# Sample custom environment for testing
RUN pip3 install numpy 2>/dev/null || true
