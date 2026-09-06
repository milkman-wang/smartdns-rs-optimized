/* Linux loopback A-record benchmark and deterministic UDP upstream.
 * See router-20260906.md for the workload, CPU placement and limitations.
 */
#include <arpa/inet.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

static unsigned domains = 256;
static double sent[65536];
static unsigned names[65536];
static unsigned long histogram[100001];

static double now(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec + t.tv_nsec * 1e-9;
}

static int question(unsigned char *buffer, unsigned id, unsigned name)
{
    memset(buffer, 0, 64);
    buffer[0] = id >> 8;
    buffer[1] = id;
    buffer[2] = 1;
    buffer[5] = 1;
    char label[20];
    int length = sprintf(label, "host%03u", name % domains);
    int pos = 12;
    buffer[pos++] = length;
    memcpy(buffer + pos, label, length);
    pos += length;
    buffer[pos++] = 5;
    memcpy(buffer + pos, "bench", 5);
    pos += 5;
    buffer[pos++] = 0;
    buffer[pos++] = 0;
    buffer[pos++] = 1;
    buffer[pos++] = 0;
    buffer[pos++] = 1;
    return pos;
}

static long cpu_ticks(int pid)
{
    char path[64], line[4096];
    sprintf(path, "/proc/%d/stat", pid);
    FILE *file = fopen(path, "r");
    if (!file)
        return -1;
    if (!fgets(line, sizeof(line), file)) {
        fclose(file);
        return -1;
    }
    fclose(file);
    char *save;
    char *value = strtok_r(strrchr(line, ')') + 2, " ", &save);
    long ticks = 0;
    for (int field = 3; value; field++, value = strtok_r(NULL, " ", &save)) {
        if (field == 14 || field == 15)
            ticks += atol(value);
    }
    return ticks;
}

static long rss_kb(int pid)
{
    char path[64], line[256];
    sprintf(path, "/proc/%d/status", pid);
    FILE *file = fopen(path, "r");
    if (!file)
        return -1;
    long rss = -1;
    while (fgets(line, sizeof(line), file)) {
        if (sscanf(line, "VmRSS: %ld", &rss) == 1)
            break;
    }
    fclose(file);
    return rss;
}

static void serve(int fd)
{
    unsigned char buffer[4096];
    /* Only benchmark queries are accepted: one question, no EDNS. */
    const unsigned char answer[] = {
        0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0x0e, 0x10, 0, 4, 192, 0, 2, 1
    };
    for (;;) {
        struct sockaddr_in peer;
        socklen_t peer_length = sizeof(peer);
        int length = recvfrom(fd, buffer, sizeof(buffer) - sizeof(answer), 0,
                              (void *)&peer, &peer_length);
        if (length < 12)
            continue;
        buffer[2] = 0x81;
        buffer[3] = 0x80;
        buffer[6] = 0;
        buffer[7] = 1;
        buffer[8] = buffer[9] = buffer[10] = buffer[11] = 0;
        memcpy(buffer + length, answer, sizeof(answer));
        sendto(fd, buffer, length + sizeof(answer), 0, (void *)&peer, peer_length);
    }
}

int main(int argc, char **argv)
{
    if (argc < 3) {
        fprintf(stderr, "Usage: %s server PORT | client PORT SECONDS WINDOW PID [DOMAINS]\n", argv[0]);
        return 2;
    }
    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    int buffer_size = 4 * 1024 * 1024;
    setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &buffer_size, sizeof(buffer_size));
    struct sockaddr_in address = {
        .sin_family = AF_INET,
        .sin_port = htons(atoi(argv[2])),
        .sin_addr.s_addr = htonl(INADDR_LOOPBACK),
    };
    if (!strcmp(argv[1], "server")) {
        if (bind(fd, (void *)&address, sizeof(address)))
            return 3;
        serve(fd);
    }
    if (argc < 6)
        return 2;
    double duration = atof(argv[3]);
    int window = atoi(argv[4]);
    int pid = atoi(argv[5]);
    if (argc > 6)
        domains = atoi(argv[6]);
    if (window < 1 || window > 1024 || domains == 0 || duration <= 0)
        return 2;
    if (connect(fd, (void *)&address, sizeof(address)))
        return 3;

    unsigned sequence = 0;
    unsigned long success = 0, errors = 0, timeouts = 0;
    int active = 0;
    long start_ticks = cpu_ticks(pid);
    double start = now(), deadline = start + duration, total_latency = 0;
    unsigned char buffer[4096];
    while (now() < deadline || active) {
        double time = now();
        while (time < deadline && active < window) {
            unsigned id = sequence++ & 65535;
            int length = question(buffer, id, sequence);
            names[id] = sequence;
            sent[id] = now();
            if (send(fd, buffer, length, 0) != length) {
                perror("send");
                return 4;
            }
            active++;
        }
        struct pollfd event = {.fd = fd, .events = POLLIN};
        int ready = poll(&event, 1, 10);
        if (ready > 0) {
            int length = recv(fd, buffer, sizeof(buffer), 0);
            if (length >= 12) {
                unsigned id = ((unsigned)buffer[0] << 8) | buffer[1];
                if (sent[id]) {
                    double us = (now() - sent[id]) * 1e6;
                    sent[id] = 0;
                    active--;
                    unsigned char expected[64];
                    int query_length = question(expected, id, names[id]);
                    if (length < query_length + 16 || !(buffer[2] & 0x80)
                        || (buffer[3] & 15) || buffer[6] || buffer[7] != 1
                        || memcmp(buffer + 12, expected + 12, query_length - 12)
                        || memcmp(buffer + length - 4, "\xc0\x00\x02\x01", 4)) {
                        errors++;
                    } else {
                        success++;
                        total_latency += us;
                        unsigned bucket = us < 100000 ? (unsigned)us : 100000;
                        histogram[bucket]++;
                    }
                } else {
                    errors++;
                }
            } else {
                errors++;
            }
        }
        if (ready == 0) {
            time = now();
            for (unsigned i = 0; i < 65536; i++) {
                if (sent[i] && time - sent[i] > 0.5) {
                    sent[i] = 0;
                    active--;
                    timeouts++;
                }
            }
        }
    }
    double elapsed = now() - start;
    long end_ticks = cpu_ticks(pid);
    unsigned long cumulative = 0;
    unsigned p50 = 0, p95 = 0, p99 = 0;
    for (unsigned i = 0; i <= 100000; i++) {
        cumulative += histogram[i];
        if (!p50 && cumulative >= success * .50)
            p50 = i;
        if (!p95 && cumulative >= success * .95)
            p95 = i;
        if (!p99 && cumulative >= success * .99)
            p99 = i;
    }
    printf("{\"qps\":%.1f,\"success\":%lu,\"errors\":%lu,\"timeouts\":%lu,"
           "\"seconds\":%.3f,\"p50_us\":%u,\"p95_us\":%u,\"p99_us\":%u,"
           "\"avg_us\":%.1f,\"server_cpu_pct\":%.1f,\"server_rss_kb\":%ld}\n",
           success / elapsed, success, errors, timeouts, elapsed, p50, p95, p99,
           success ? total_latency / success : 0,
           100.0 * (end_ticks - start_ticks) / sysconf(_SC_CLK_TCK) / elapsed,
           rss_kb(pid));
    return errors || timeouts ? 1 : 0;
}
