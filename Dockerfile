FROM debian:stable-20230109-slim
RUN apt-get update && apt-get install libmariadb-dev -y
COPY target/release/server /ultrafinance
COPY frontend/dist /frontend/dist
EXPOSE 3000
CMD ["/ultrafinance"]
