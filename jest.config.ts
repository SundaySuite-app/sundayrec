import type { Config } from 'jest'

const config: Config = {
  preset: 'ts-jest',
  testEnvironment: 'node',
  roots: ['<rootDir>/tests'],
  moduleNameMapper: {
    '^@shared/(.*)$': '<rootDir>/src/shared/$1',
    '^electron$': '<rootDir>/__mocks__/electron.ts',
    '^electron-store$': '<rootDir>/__mocks__/electron-store.ts',
  },
  globals: {
    'ts-jest': {
      tsconfig: { esModuleInterop: true }
    }
  }
}

export default config
