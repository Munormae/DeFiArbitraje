import { ethers } from "hardhat";

async function main() {
  const permit2 = process.env.PERMIT2_ADDRESS || "0x000000000022D473030F116dDEE9F6B43aC78BA3";
  const Exec = await ethers.getContractFactory("Executor");
  const exec = await Exec.deploy(permit2);
  await exec.waitForDeployment();
  console.log("Executor deployed at:", await exec.getAddress());
}
main().catch((e) => { console.error(e); process.exit(1); });
