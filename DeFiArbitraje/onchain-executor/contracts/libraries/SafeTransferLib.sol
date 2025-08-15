// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

library SafeTransferLib {
    function safeTransfer(address token, address to, uint256 value) internal {
        (bool s, bytes memory d) = token.call(abi.encodeWithSelector(0xa9059cbb, to, value));
        require(s && (d.length == 0 || abi.decode(d, (bool))), "TRANSFER_FAILED");
    }
    function safeTransferFrom(address token, address from, address to, uint256 value) internal {
        (bool s, bytes memory d) = token.call(abi.encodeWithSelector(0x23b872dd, from, to, value));
        require(s && (d.length == 0 || abi.decode(d, (bool))), "TRANSFER_FROM_FAILED");
    }
    function safeApprove(address token, address spender, uint256 value) internal {
        (bool s, bytes memory d) = token.call(abi.encodeWithSelector(0x095ea7b3, spender, value));
        require(s && (d.length == 0 || abi.decode(d, (bool))), "APPROVE_FAILED");
    }
    function balanceOf(address token, address account) internal view returns (uint256) {
        (bool s, bytes memory d) = token.staticcall(abi.encodeWithSelector(0x70a08231, account));
        require(s && d.length >= 32, "BALANCE_FAILED");
        return abi.decode(d, (uint256));
    }
}
