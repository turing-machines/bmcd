// This header file corresponds to the c_interface module. Refer to
// c_interface.rs for more detailed function description.

#pragma once

#include <stdint.h>

void tpi_initialize(void);
void tpi_node_power(int num, int status);
int tpi_usb_mode(int mode, int node);
int tpi_usb_mode_v2(int mode, int node, bool boot_pin);
int tpi_get_node_power(int node);
void tpi_rtl_reset();
void tpi_reset_node(int node);

typedef enum {
    FR_SUCCESS,
    FR_INVALID_ARGS,
    FR_DEVICE_NOT_FOUND,
    FR_GPIO_ERROR,
    FR_USB_ERROR,
    FR_IO_ERROR,
    FR_TIMEOUT,
    FR_CHECKSUM_MISMATCH,
    FR_OTHER,
} flashing_result;

flashing_result tpi_flash_node(int node_id, const char* image_path);
void tpi_node_to_msd(int node_id);
void tpi_clear_usbboot();

