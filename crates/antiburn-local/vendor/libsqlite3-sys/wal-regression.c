/* Test the same amalgamation that the bundled Cargo build compiles. */
#include "sqlite3/sqlite3.c"
#include <stdio.h>

#define CHECK(condition) do { \
  if(!(condition)){ \
    fprintf(stderr, "Check failed at line %d: %s\n", __LINE__, #condition); \
    exit(1); \
  } \
} while(0)

static int expected_size;
static sqlite3_int64 expected_offset;
static int read_result;

static int read_frame(sqlite3_file *file, void *buffer, int size, sqlite3_int64 offset){
  (void)file;
  CHECK(size == expected_size);
  CHECK(offset == expected_offset);
  memset(buffer, 0x42, size);
  return read_result;
}

int main(void){
  sqlite3_io_methods methods = {0};
  sqlite3_file file = {0};
  Wal wal = {0};
  unsigned char output[65537];
  const int sizes[] = {512, 1024, 4096, 32768, 65536};
  unsigned int a, b;
  int i, error;
  methods.iVersion = 1;
  methods.xRead = read_frame;
  file.pMethods = &methods;
  wal.pWalFd = &file;
  for(a=0; a<sizeof(sizes)/sizeof(sizes[0]); a++){
    for(b=0; b<sizeof(sizes)/sizeof(sizes[0]); b++){
      for(error=0; error<2; error++){
        int page = sizes[a];
        int out = sizes[b];
        wal.hdr.szPage = page == 65536 ? 1 : page;
        expected_size = page < out ? page : out;
        expected_offset = 32 + 2 * (24 + (sqlite3_int64)page) + 24;
        read_result = error ? SQLITE_IOERR_READ : SQLITE_OK;
        memset(output, 0xa5, sizeof(output));
        CHECK(sqlite3WalReadFrame(&wal, 3, out, output) == read_result);
        for(i=0; i<expected_size; i++) CHECK(output[i] == 0x42);
        for(i=expected_size; i<out; i++) CHECK(output[i] == 0);
        for(i=out; i<(int)sizeof(output); i++) CHECK(output[i] == 0xa5);
      }
    }
  }
  puts("WAL regression: 50 size/error cases passed");
  return 0;
}
