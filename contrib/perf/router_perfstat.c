/* Existing-thread user-space PMU counters; CLI: PID SECONDS. */
#define _GNU_SOURCE
#include <dirent.h>
#include <linux/perf_event.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/syscall.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    char path[96];
    snprintf(path, sizeof(path), "/proc/%d/task", atoi(argv[1]));
    DIR *dir = opendir(path);
    if (!dir) { perror("tasks"); return 1; }
    int fds[256][4], count = 0;
    uint64_t configs[] = {PERF_COUNT_HW_CPU_CYCLES, PERF_COUNT_HW_INSTRUCTIONS,
                          PERF_COUNT_HW_CACHE_MISSES, PERF_COUNT_HW_BRANCH_MISSES};
    struct dirent *entry;
    while ((entry = readdir(dir)) && count < 256) {
        int tid = atoi(entry->d_name);
        if (!tid) continue;
        for (int metric = 0; metric < 4; metric++) {
            struct perf_event_attr attr = {0};
            attr.type = PERF_TYPE_HARDWARE;
            attr.size = sizeof(attr);
            attr.config = configs[metric];
            attr.disabled = 1;
            attr.exclude_kernel = 1;
            attr.exclude_hv = 1;
            attr.read_format = PERF_FORMAT_TOTAL_TIME_ENABLED | PERF_FORMAT_TOTAL_TIME_RUNNING;
            fds[count][metric] = syscall(SYS_perf_event_open, &attr, tid, -1, -1, 0);
            if (fds[count][metric] < 0) { perror("perf_event_open"); return 1; }
        }
        count++;
    }
    closedir(dir);
    for (int t = 0; t < count; t++) for (int m = 0; m < 4; m++) ioctl(fds[t][m], PERF_EVENT_IOC_ENABLE, 0);
    usleep((useconds_t)(atof(argv[2])*1000000));
    double totals[4] = {0};
    for (int t = 0; t < count; t++) for (int m = 0; m < 4; m++) {
        uint64_t data[3];
        ioctl(fds[t][m], PERF_EVENT_IOC_DISABLE, 0);
        if (read(fds[t][m], data, sizeof(data)) != sizeof(data)) return 1;
        if (data[2]) totals[m] += (double)data[0]*data[1]/data[2];
        close(fds[t][m]);
    }
    printf("{\"threads\":%d,\"user_cycles\":%.0f,\"user_instructions\":%.0f,\"cache_misses\":%.0f,\"branch_misses\":%.0f}\n", count,totals[0],totals[1],totals[2],totals[3]);
}
