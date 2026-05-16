const ElectronStore = jest.fn().mockImplementation(() => {
  let data: Record<string, unknown> = {}
  return {
    get: jest.fn((key: string) => data[key]),
    set: jest.fn((key: string, val: unknown) => { data[key] = val }),
    clear: jest.fn(() => { data = {} }),
    get store() { return data },
    set store(val: Record<string, unknown>) { data = { ...val } },
  }
})

export default ElectronStore
