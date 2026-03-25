# Somnia Reactive Message dApp

A minimal reactive message board dApp built for Somnia network using Foundry and React.

## Repository Structure

```
├── contracts/              # Foundry smart contract project
│   ├── src/
│   │   └── MessageBoard.sol
│   ├── test/
│   │   └── MessageBoard.t.sol
│   ├── foundry.toml
│   └── README.md
├── frontend/               # React + Vite frontend
│   ├── src/
│   │   ├── abi/
│   │   │   └── MessageBoardABI.ts
│   │   ├── App.tsx
│   │   ├── App.css
│   │   ├── MessageProvider.tsx
│   │   ├── main.tsx
│   │   ├── somnia.ts
│   │   ├── chain.ts
│   │   └── wagmi.ts
│   ├── index.html
│   ├── vite.config.ts
│   ├── tsconfig.json
│   ├── tsconfig.node.json
│   ├── package.json
│   ├── .gitignore
│   └── README.md
└── README.md               # This file
```

## Quick Start

### Deploy the Contract

```bash
cd contracts
forge install foundry-rs/forge-std --no-git
forge build
forge test
forge create MessageBoard --rpc-url https://dream-rpc.somnia.network/ --private-key <PRIVATE_KEY>
```

Copy the deployed contract address.

### Configure and Run Frontend

1. Update `frontend/src/MessageProvider.tsx` with the deployed address:
   ```typescript
   const MESSAGE_BOARD_ADDRESS = '0xYOUR_DEPLOYED_ADDRESS'
   ```

2. Install dependencies and start:
   ```bash
   cd frontend
   npm install
   npm run dev
   ```

3. Open http://localhost:5173 in your browser with MetaMask configured for Somnia Testnet (Chain ID: 50312).

## Technology Stack

- **Smart Contracts:** Solidity 0.8.20, Foundry
- **Frontend:** React 18, TypeScript, Vite
- **Web3:** viem, wagmi, @tanstack/react-query
- **Reactivity:** Somnia Reactivity SDK (WebSocket subscriptions)

## Features

- Real-time message feed using Somnia's event-driven reactivity
- Wallet connection via MetaMask
- Post messages up to 500 characters
- Clean, minimal, production-oriented code structure

## Important Notes

- Somnia Reactivity requires WebSocket RPC endpoints (already configured)
- Replace the placeholder contract address before using the frontend
- The app is designed for Somnia Testnet (Chain ID: 50312)

## License

MIT
