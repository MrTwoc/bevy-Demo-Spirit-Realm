/*  区块管理器：
    一、玩家区块加载逻辑：
        区块加载数量为玩家设置的可视距离，设置为4，则是16个区块，玩家在地图上移动时，会加载16个区块
        需要先把玩家的世界坐标转成区块坐标，然后以玩家所在区块为中心向四周加载区块。
        1. 计算玩家所在区块的区块坐标   例如：玩家坐标为(320, 320, 320) 
            区块坐标为(玩家坐标 ÷ 单个区块大小 = [320,320,320] ÷ 32) => (10, 10, 10)
        2. 获得玩家可视距离，例如：可视距离为4，则 9x9x9 个区块： (10-4, 10-4, 10-4) => (6, 6, 6) 到 (10+4, 10+4, 10+4) => (14, 14, 14)
            注：总数为奇数，确保玩家所在区块为中心，加载9x9x9个区块
        3. 区块坐标转世界坐标，用于获得噪声值，生成具体地形
            例如：
            x,y,z (6, 6, 6) => (6x32, 6x32, 6x32) => (192, 192, 192)
            x,y,z (14, 14, 14) => (14x32, 14x32, 14x32) => (448,448,448)
        4. 将需要加载的区块坐标，分段加载，并拼接
            计算需要加载的区块坐标，例如：
            玩家所在的区块坐标 
            X + 可视半径, Y + 可视半径, Z + 可视半径
            X - 可视半径, Y - 可视半径, Z - 可视半径

        // 遍历可视半径中所有的区块坐标: center_chunk：玩家所在区块的区块坐标
        for x in (center_chunk - 可视半径) + (center_chunk + 可视半径){
            for y in (center_chunk - 可视半径) + (center_chunk + 可视半径){
                for z in (center_chunk - 可视半径) + (center_chunk + 可视半径){
                    // 计算区块坐标转世界坐标，用于获得噪声值，生成具体地形
                    (这里可以实现加载区块的同时，卸载区块)
                    spawn_chunk(x, y, z);
                    unload_chunk(x, y, z);
                }
            }    
        }

    二、玩家区块卸载逻辑：
        维护一个数组，数组长度 = 玩家可视半径中的所有区块，每个区块都有一个索引，索引从0开始，数组长度为 区块数量
        1. 根据玩家的区块坐标判断，数组内的区块坐标是否超过玩家的可视半径，如果超过，则卸载区块

*/

/* 
关于世界方块的偏移量：
    fn world_to_chunk_offset(world_x, world_y, world_z):
    chunk_x = int(world_x // 32)
    chunk_y = int(world_y // 32)
    chunk_z = int(world_z // 32)
    
    chunk_start_x = chunk_x * 32
    chunk_start_y = chunk_y * 32
    chunk_start_z = chunk_z * 32
    
    offset_x = world_x - chunk_start_x
    offset_y = world_y - chunk_start_y
    offset_z = world_z - chunk_start_z
    
    return offset_x, offset_y, offset_z
*/


/*
    源坐标：(998.1,456.9,789.4)
    区块坐标：chunk_x:31, chunk_y:14, chunk_z:24
    偏移量：offset_x:6, offset_y:8, offset_z:21
    真实坐标：(998,456,789)
    chunk_start_POS：992,448,768
*/
// type pos = (u32, u32, u32);
fn main(){
    // 区块坐标转世界坐标的偏移量计算
    world_to_chunk_offset(1998.1, 456.9, 789.4);       // 这里的xyz，是玩家的世界坐标
}

// 输入参数为玩家的世界坐标
fn world_to_chunk_offset(world_x:f32, world_y:f32, world_z:f32){
    // 设 玩家坐标 为 (998,456,789)
    // 求出区块坐标 = (玩家坐标 ÷ 单个区块大小 = (x / 32), (y / 32), (z / 32) ) => (31, 150, 26)
    let (chunk_x, chunk_y, chunk_z) = world_to_chunk(world_x as i32, world_y as i32, world_z as i32);

    // 计算区块起始坐标 = 区块坐标(小数向下取整) * 32
    let chunk_start_x = chunk_x * 32;
    let chunk_start_y = chunk_y * 32;
    let chunk_start_z = chunk_z * 32;

    // 算出偏移量 = 玩家真实坐标 % 区块起始坐标
    let offset_x = (world_x as i32) % chunk_start_x;
    let offset_y = (world_y as i32) % chunk_start_y;
    let offset_z = (world_z as i32) % chunk_start_z;

    // 如何再根据偏移量再算出真实坐标 ： 真实坐标 = 区块坐标 * 32 + 偏移量
    let true_world_x = chunk_x * 32 + offset_x;
    let true_world_y = chunk_y * 32 + offset_y;
    let true_world_z = chunk_z * 32 + offset_z;

    println!("源坐标：({},{},{})", world_x, world_y, world_z);
    println!("区块坐标：chunk_x:{}, chunk_y:{}, chunk_z:{}", chunk_x, chunk_y, chunk_z);
    println!("区块起始坐标：{},{},{}", chunk_start_x, chunk_start_y, chunk_start_z);
    println!("偏移量：offset_x:{}, offset_y:{}, offset_z:{}", offset_x, offset_y, offset_z);
    println!("真实坐标：({},{},{})", true_world_x, true_world_y, true_world_z);

    // println!("chunk_start_POS：{},{},{}", chunk_start_x, chunk_start_y, chunk_start_z);
}

// 世界坐标转区块坐标
fn world_to_chunk(x:i32, y:i32, z:i32) -> (i32, i32, i32) {
    return (x / 32 , y / 32, z / 32);
}

