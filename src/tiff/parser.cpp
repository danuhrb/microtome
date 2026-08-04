#include <cstddef>
#include <cstdint>
#include "tiff.h"

inline size_t typeSize(uint16_t t) {
switch(t) {
case 1: case 2: case 6: case 7:    return 1; // BYTE ASCII SBYTE UNDEFINED
case 3: case 8:                    return 2; // SHORT SSHORT
case 4: case 9: case 11: case 13:  return 4; // LONG SLONG FLOAT IFD
case 5: case 10: case 12: case 16:
case 17: case 18:                  return 8; // RATIONAL DOUBLE LONG8 ...
default: return 0; // unknown -> skip payload
    }
};